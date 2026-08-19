"""
Ingest daily option chain snapshots from Databento (OPRA.PILLAR).

For each business date, per root:
  - fetch instrument definitions for the root (strikes, expiries, C/P)
  - prefilter the chain, so quotes are only requested for contracts we keep
  - fetch a 1-minute NBBO snapshot (cbbo-1m) at 15:45 ET
  - join, compute mids, write a raw parquet partition and upsert into DuckDB

Cost control: cbbo-1m restricted to a single minute per day is cheap on
pay-as-you-go historical. cbbo-1m history goes back to 2013-04-01.
For multi-year backfills, prefer client.batch.submit_job over get_range.

Days already ingested are skipped (see options_db.done_dates), so an
interrupted backfill can be resumed without paying for them twice.

Usage:
    export DATABENTO_API_KEY=...
    uv run python ingest_databento.py --root SPY --start 2024-01-02 --end 2024-12-31
    uv run python ingest_databento.py --root SPY --start 2024-01-03 --preview-cost
"""

from __future__ import annotations

import argparse
import datetime as dt
import os
import sys
from zoneinfo import ZoneInfo

import databento as db
import pandas as pd

import options_db

DATASET = "OPRA.PILLAR"
SNAPSHOT_ET = dt.time(15, 45)      # snapshot minute, US/Eastern

# ---- chain prefilter (cost control): applied to definitions BEFORE
# ---- requesting quotes, so we only pay for contracts we'll use
MIN_TTE = 7 / 365.25               # drop 0DTE / ultra-short
MAX_TTE = 2.5                      # drop LEAPS beyond grid horizon
MONEYNESS_BAND = (0.5, 1.5)        # strike / rough-spot
KEEP_ALL_EXPIRIES_UNDER = 60 / 365.25  # weeklies matter at the short end
SYMBOL_CHUNK = 1500                # max raw symbols per API request

# Consecutive failures tolerated before aborting. A bad key or an exhausted
# quota fails identically on every date; without this the run would print 252
# "skipped" lines and leave an empty store looking like a calendar problem.
MAX_CONSECUTIVE_ERRORS = 3

ET = ZoneInfo("America/New_York")


def parse_osi(raw_symbol: str) -> dict:
    """Parse OCC/OSI symbology: 'SPY   240119C00480000'
    -> root, expiry, right, strike. The tail is always 15 chars (6 date +
    1 right + 8 strike), so this holds for any root length."""
    s = raw_symbol.strip()
    tail = s[-15:]
    return {
        "root": s[:-15].strip(),
        "expiry": dt.date(2000 + int(tail[0:2]), int(tail[2:4]), int(tail[4:6])),
        "opt_right": tail[6],                 # 'C' or 'P'
        "strike": int(tail[7:]) / 1000.0,
    }


def is_monthly(expiry: dt.date) -> bool:
    """Standard monthly = 3rd Friday. (Holiday-shifted Thursdays slip
    through as non-monthly; acceptable loss for a prefilter.)"""
    return expiry.weekday() == 4 and 15 <= expiry.day <= 21


def rough_spot(defs: pd.DataFrame) -> float:
    """Median strike of the shortest listed expiry. Weeklies list strikes
    tightly around current spot, so this is a solid proxy for a wide
    moneyness prefilter -- no extra data request needed. If you want an
    exact spot, fetch one ohlcv-1d bar for the underlying from EQUS.MINI."""
    near = defs[defs["expiry"] == defs["expiry"].min()]
    src = near if len(near) >= 5 else defs
    return float(src["strike"].median())


def filter_chain(defs: pd.DataFrame, date: dt.date) -> pd.DataFrame:
    tte = (pd.to_datetime(defs["expiry"]) - pd.Timestamp(date)).dt.days / 365.25
    spot = rough_spot(defs)
    keep_tenor = tte.between(MIN_TTE, MAX_TTE)
    keep_expiry = (tte < KEEP_ALL_EXPIRIES_UNDER) | defs["expiry"].map(is_monthly)
    keep_strike = (defs["strike"] / spot).between(*MONEYNESS_BAND)
    return defs[keep_tenor & keep_expiry & keep_strike]


def quote_cost(client: db.Historical, symbols: list[str], snap: dt.datetime) -> float:
    """Price a quote request before making it. `metadata.get_cost` is a free
    metadata call, so this never adds to the bill it is reporting on."""
    try:
        return float(client.metadata.get_cost(
            dataset=DATASET, schema="cbbo-1m", stype_in="raw_symbol",
            symbols=symbols, start=snap, end=snap + dt.timedelta(minutes=1),
        ))
    except Exception:  # noqa: BLE001 — never let a pricing probe stop a run
        return float("nan")


def definition_cost(client: db.Historical, root: str, date: dt.date) -> float:
    """Same, for the definition request. Definitions are billed too — easy to
    forget when budgeting, since the prefilter only shrinks the quote leg."""
    try:
        return float(client.metadata.get_cost(
            dataset=DATASET, schema="definition", stype_in="parent",
            symbols=[f"{root}.OPT"], start=date.isoformat(),
            end=(date + dt.timedelta(days=1)).isoformat(),
        ))
    except Exception:  # noqa: BLE001
        return float("nan")


def fetch_definitions(client: db.Historical, root: str, date: dt.date) -> pd.DataFrame:
    defs = client.timeseries.get_range(
        dataset=DATASET,
        schema="definition",
        stype_in="parent",
        symbols=[f"{root}.OPT"],
        start=date.isoformat(),
        end=(date + dt.timedelta(days=1)).isoformat(),
    ).to_df()
    if defs.empty:
        return defs
    # to_df() returns a timestamp index, and dropping duplicates keeps those
    # labels — so the parsed frame inherits a non-unique index and concat fails
    # trying to align on it. Reset once, up front, so both share a RangeIndex.
    defs = (
        defs[["instrument_id", "raw_symbol"]]
        .drop_duplicates("instrument_id")
        .reset_index(drop=True)
    )
    meta = defs["raw_symbol"].apply(parse_osi).apply(pd.Series)
    return pd.concat([defs, meta], axis=1)


def fetch_day(
    client: db.Historical, root: str, date: dt.date, budget: dict | None = None
) -> pd.DataFrame | None:
    snap_start = dt.datetime.combine(date, SNAPSHOT_ET, tzinfo=ET)
    snap_end = snap_start + dt.timedelta(minutes=1)

    d_cost = definition_cost(client, root, date)
    print(f"  {date} {root}: definitions ${d_cost:.6f}")
    if budget is not None:
        budget["spent"] += 0.0 if d_cost != d_cost else d_cost

    defs = fetch_definitions(client, root, date)
    if defs.empty:
        return None

    keep = filter_chain(defs, date)
    if keep.empty:
        return None
    print(f"  {date} {root}: chain {len(defs)} -> {len(keep)} after prefilter")

    symbols = keep["raw_symbol"].str.strip().tolist()
    q_cost = quote_cost(client, symbols[:SYMBOL_CHUNK], snap_start)
    print(f"  {date} {root}: quotes ${q_cost:.6f} for {len(symbols)} contracts")
    if budget is not None:
        budget["spent"] += 0.0 if q_cost != q_cost else q_cost
        if budget["cap"] is not None and budget["spent"] > budget["cap"]:
            raise RuntimeError(
                f"estimated spend ${budget['spent']:.4f} exceeds --max-cost "
                f"${budget['cap']:.4f}"
            )

    frames = []
    for i in range(0, len(symbols), SYMBOL_CHUNK):
        q = client.timeseries.get_range(
            dataset=DATASET,
            schema="cbbo-1m",
            stype_in="raw_symbol",
            symbols=symbols[i:i + SYMBOL_CHUNK],
            start=snap_start,
            end=snap_end,
        ).to_df()
        if not q.empty:
            frames.append(q.reset_index())
    if not frames:
        return None
    quotes = pd.concat(frames, ignore_index=True)

    # Last *row* per instrument in the window. groupby().last() takes the last
    # non-null value per column independently, which can pair a bid from one
    # timestamp with an ask from another.
    quotes = (
        quotes.sort_values("ts_event")
        .drop_duplicates("instrument_id", keep="last")
        .reset_index(drop=True)
    )

    bid, ask = quotes["bid_px_00"], quotes["ask_px_00"]
    mid = (bid + ask) / 2.0
    quotes = quotes.assign(
        bid=bid,
        ask=ask,
        mid=mid,
        # A two-sided market is required: 0/0 on an unquoted strike would be
        # NaN, and a one-sided book makes the spread meaningless.
        rel_spread=((ask - bid) / mid).where(mid > 0),
        snapshot_ts=quotes["ts_event"],
    )
    quotes = quotes[(bid > 0) & (ask > 0) & (ask >= bid)]
    if quotes.empty:
        return None
    quotes = quotes[["instrument_id", "bid", "ask", "mid", "rel_spread", "snapshot_ts"]]

    out = quotes.merge(keep, on="instrument_id", how="inner")
    out["quote_date"] = date
    out["tte"] = (
        pd.to_datetime(out["expiry"]) - pd.Timestamp(date)
    ).dt.days / 365.25
    return out


def check_scale(df: pd.DataFrame, date: dt.date) -> None:
    """DBN prices are fixed-point 1e-9. Recent databento-python converts in
    to_df(), but a version that does not would silently store dollar values
    scaled by a billion — cheap to catch, expensive to discover later."""
    hi = float(df["mid"].max())
    if hi > 1e5:
        print(
            f"  !! {date}: max mid {hi:.3g} looks like raw fixed-point, not "
            f"dollars — check the databento-python price_type before continuing",
            file=sys.stderr,
        )


def preview_cost(client: db.Historical, root: str, sample_date: dt.date) -> None:
    """Dry-run pricing for one day's filtered quote request. Excludes the
    definition request, which is billed on every date too, so treat the
    x252 extrapolation as a floor rather than a budget."""
    snap = dt.datetime.combine(sample_date, SNAPSHOT_ET, tzinfo=ET)
    defs = fetch_definitions(client, root, sample_date)
    if defs.empty:
        print(f"no definitions for {root} on {sample_date}")
        return
    keep = filter_chain(defs, sample_date)
    cost = client.metadata.get_cost(
        dataset=DATASET, schema="cbbo-1m", stype_in="raw_symbol",
        symbols=keep["raw_symbol"].str.strip().tolist()[:SYMBOL_CHUNK],
        start=snap, end=snap + dt.timedelta(minutes=1),
    )
    n = min(len(keep), SYMBOL_CHUNK)
    print(f"quote cost for {root} {sample_date} (first chunk, {n} symbols): ${cost}")
    print(f"  ~${cost * 252:.2f} for 252 sessions, excluding definitions")


def run(client, con, root: str, dates, force: bool, budget: dict) -> None:
    done = set() if force else options_db.done_dates(con, root)
    todo = [d for d in dates if d not in done]
    print(f"{root}: {len(todo)} sessions to ingest ({len(dates) - len(todo)} already done)")

    consecutive = 0
    for d in todo:
        try:
            df = fetch_day(client, root, d, budget)
        except Exception as e:  # noqa: BLE001 — classified below
            consecutive += 1
            options_db.record(con, d, root, "error", detail=str(e)[:500])
            print(f"  {d} {root}: ERROR {e}", file=sys.stderr)
            if consecutive >= MAX_CONSECUTIVE_ERRORS:
                print(
                    f"aborting after {consecutive} consecutive failures — this "
                    f"is a credentials/quota problem, not a calendar one",
                    file=sys.stderr,
                )
                return
            continue

        consecutive = 0
        if df is None or df.empty:
            # Holiday, half-day (15:45 is after a 13:00 close), or a chain that
            # the prefilter emptied. Recorded so a rerun does not re-request it.
            options_db.record(con, d, root, "empty")
            print(f"  {d} {root}: no quotes")
            continue

        check_scale(df, d)
        options_db.write_partition(
            con, df.reindex(columns=options_db.COLUMNS), d, root
        )
        n = options_db.write_quotes(con, df)
        options_db.record(con, d, root, "ok", contracts=n)
        print(f"  {d} {root}: {n} quotes")


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--root", action="append", default=None,
                   help="underlying root; repeatable (default: SPY)")
    p.add_argument("--start", required=True, help="first session, YYYY-MM-DD")
    p.add_argument("--end", help="last session, YYYY-MM-DD (default: --start)")
    p.add_argument("--force", action="store_true",
                   help="re-ingest days already recorded (re-bills them)")
    p.add_argument("--max-cost", type=float, default=None,
                   help="abort once estimated spend exceeds this many dollars")
    p.add_argument("--preview-cost", action="store_true",
                   help="price one day's request and exit without ingesting")
    args = p.parse_args()

    key = os.environ.get("DATABENTO_API_KEY")
    if not key:
        print("DATABENTO_API_KEY is not set", file=sys.stderr)
        return 2

    roots = args.root or ["SPY"]
    start = dt.date.fromisoformat(args.start)
    end = dt.date.fromisoformat(args.end) if args.end else start
    dates = [d.date() for d in pd.bdate_range(start, end)]

    client = db.Historical(key)

    if args.preview_cost:
        for root in roots:
            preview_cost(client, root, start)
        return 0

    con = options_db.connect()
    try:
        budget = {"spent": 0.0, "cap": args.max_cost}
        for root in roots:
            run(client, con, root, dates, args.force, budget)
        print(f"estimated spend this run: ${budget['spent']:.6f}")
        out = options_db.export(con)
        print(f"exported {out}: {options_db.summary(con)}")
    finally:
        con.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
