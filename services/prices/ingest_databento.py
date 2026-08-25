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
import concurrent.futures as cf
import datetime as dt
import os
import sys
import threading
import time
from collections import deque
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

# Sessions are independent and the loop is latency-bound (~6 round trips of
# ~4-5s each), not bandwidth-bound, so concurrency buys close to a linear
# speed-up. Kept modest to stay well inside Databento's rate limits.
DEFAULT_WORKERS = 6

ET = ZoneInfo("America/New_York")


class BudgetExceeded(RuntimeError):
    """Estimated spend passed --max-cost. Aborts the run rather than being
    recorded as a per-date failure: the cap is a stop, not a bad session."""


_budget_lock = threading.Lock()
_local = threading.local()


def client_for(key: str) -> db.Historical:
    """One client per worker thread — the HTTP session underneath is not
    documented as thread-safe, and a client is cheap."""
    c = getattr(_local, "client", None)
    if c is None:
        c = _local.client = db.Historical(key)
    return c


def charge(budget: dict | None, amount: float) -> None:
    if budget is None:
        return
    with _budget_lock:
        budget["spent"] += 0.0 if amount != amount else amount   # NaN-safe
        if budget["cap"] is not None and budget["spent"] > budget["cap"]:
            raise BudgetExceeded(
                f"estimated spend ${budget['spent']:.4f} exceeds --max-cost "
                f"${budget['cap']:.4f}"
            )


def is_no_chain(exc: Exception) -> bool:
    """A market holiday has no listed chain, but Databento reports that as a
    422 on the parent symbol rather than an empty frame. Classifying it as a
    failure would both mislabel the session and, because `done_dates` excludes
    errors so they can be retried, park every holiday at the head of the next
    run's todo list — where MAX_CONSECUTIVE_ERRORS aborts it before it reaches
    real work."""
    return "symbology_invalid_request" in str(exc)


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
) -> tuple[pd.DataFrame | None, str]:
    snap_start = dt.datetime.combine(date, SNAPSHOT_ET, tzinfo=ET)
    snap_end = snap_start + dt.timedelta(minutes=1)

    d_cost = definition_cost(client, root, date)
    charge(budget, d_cost)

    defs = fetch_definitions(client, root, date)
    if defs.empty:
        return None, "no definitions"

    keep = filter_chain(defs, date)
    if keep.empty:
        return None, f"chain {len(defs)} -> 0 after prefilter"

    symbols = keep["raw_symbol"].str.strip().tolist()
    chunks = [symbols[i:i + SYMBOL_CHUNK] for i in range(0, len(symbols), SYMBOL_CHUNK)]
    # Price one chunk and scale. Billing is exactly linear in record count
    # (verified: 10 venue records per contract, cost strictly proportional), so
    # this is as accurate as pricing every chunk while costing one round trip
    # instead of three — and this loop is latency-bound, so those round trips
    # are precisely what is worth removing. Pricing the first chunk *without*
    # scaling was the earlier bug: it under-reported any chain past 1500.
    sample = chunks[0]
    unit = quote_cost(client, sample, snap_start) / len(sample)
    q_cost = unit * len(symbols)
    charge(budget, q_cost)

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
        return None, f"{len(symbols)} contracts, no quotes returned"
    quotes = pd.concat(frames, ignore_index=True)

    # Last *row* per instrument in the window. groupby().last() takes the last
    # non-null value per column independently, which can pair a bid from one
    # timestamp with an ask from another.
    #
    # Ordered by ts_recv (the interval close, always populated) rather than
    # ts_event, which is the last book *update* and is null for any interval
    # where nothing traded — most of an option chain. Sorting on it put those
    # nulls last, so keep="last" preferred the row with no timestamp. ts_event
    # only breaks ties, with nulls first so a real update wins.
    quotes = (
        quotes.sort_values(["ts_recv", "ts_event"], na_position="first")
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
        # The interval this quote closes, not the last book update: ts_event is
        # null on any contract that went unquoted through the minute.
        snapshot_ts=quotes["ts_recv"],
        bid_size=quotes["bid_sz_00"],
        ask_size=quotes["ask_sz_00"],
        # Kept precisely *because* it is often null: ts_event is the last book
        # update, so a null (or an old one) marks a quote carried forward
        # through the snapshot minute rather than set during it. That is the
        # staleness signal the surface fit needs to down-weight a point;
        # `rel_spread` cannot see it. Free — it rides along in the same payload.
        last_update_ts=quotes["ts_event"],
    )
    quotes = quotes[(bid > 0) & (ask > 0) & (ask >= bid)]
    if quotes.empty:
        return None, f"{len(symbols)} contracts, none two-sided"
    quotes = quotes[[
        "instrument_id", "bid", "ask", "mid", "rel_spread", "snapshot_ts",
        "bid_size", "ask_size", "last_update_ts",
    ]]

    out = quotes.merge(keep, on="instrument_id", how="inner")
    out["quote_date"] = date
    out["tte"] = (
        pd.to_datetime(out["expiry"]) - pd.Timestamp(date)
    ).dt.days / 365.25
    return out, (f"chain {len(defs)}->{len(keep)}, {len(out)} quotes, "
                 f"${d_cost + q_cost:.6f}")


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


def run(key: str, con, root: str, dates, force: bool, budget: dict,
        workers: int) -> bool:
    """Ingest every outstanding session for `root`. Returns False if the run
    was cut short (budget cap or a systemic failure), True otherwise.

    Fetching runs on a thread pool because the work is latency-bound; writing
    stays on this thread because DuckDB takes a single writer, and serialising
    the writes also keeps `upsert`'s delete+insert transaction uncontended.
    """
    done = set() if force else options_db.done_dates(con, root)
    todo = [d for d in dates if d not in done]
    print(f"{root}: {len(todo)} sessions to ingest "
          f"({len(dates) - len(todo)} already done)")
    if not todo:
        return True

    # Ordering is meaningless once completions arrive out of order, so the
    # systemic-failure guard watches the last few *real* outcomes instead.
    # Holidays are excluded: they are a calendar fact, not a failure.
    recent: deque[bool] = deque(maxlen=MAX_CONSECUTIVE_ERRORS)
    stop = threading.Event()
    t0 = time.perf_counter()
    ok = empty = failed = 0

    def fetch(d):
        if stop.is_set():
            return d, None, None, "skipped"
        try:
            df, note = fetch_day(client_for(key), root, d, budget)
            return d, df, None, note
        except Exception as e:  # noqa: BLE001 — classified by the caller
            return d, None, e, ""

    # Submit a bounded window rather than the whole list. The pool drains
    # independently of this thread, so queueing everything up front lets it run
    # arbitrarily far ahead — and every session it starts is billed before the
    # cap can be observed here. A window of 2x workers keeps the worst-case
    # overshoot past --max-cost to a handful of sessions instead of the tail of
    # the entire run.
    queue = iter(todo)
    n = 0
    with cf.ThreadPoolExecutor(max_workers=workers) as pool:
        pending = {}

        def submit_next() -> bool:
            nxt = next(queue, None)
            if nxt is None:
                return False
            pending[pool.submit(fetch, nxt)] = nxt
            return True

        for _ in range(workers * 2):
            if not submit_next():
                break

        while pending:
            finished, _ = cf.wait(pending, return_when=cf.FIRST_COMPLETED)
            for fut in finished:
                pending.pop(fut)
                n += 1
                # Refill immediately, whatever the outcome: doing it only on the
                # success path would shrink the window on every holiday or error
                # until the pool starved.
                if not stop.is_set():
                    submit_next()
                d, df, err, note = fut.result()
                rate = (time.perf_counter() - t0) / n
                eta = dt.timedelta(seconds=round(rate * (len(todo) - n)))
                head = f"[{n}/{len(todo)} eta {eta}] {d} {root}:"

                if isinstance(err, BudgetExceeded):
                    print(f"{head} STOP {err}", file=sys.stderr)
                    stop.set()
                    break
                if err is not None and is_no_chain(err):
                    options_db.record(con, d, root, "empty", detail="market holiday")
                    empty += 1
                    print(f"{head} holiday, no chain")
                    continue
                if err is not None:
                    failed += 1
                    recent.append(False)
                    options_db.record(con, d, root, "error", detail=str(err)[:500])
                    print(f"{head} ERROR {err}", file=sys.stderr)
                    if len(recent) == recent.maxlen and not any(recent):
                        print(f"aborting after {recent.maxlen} consecutive failures "
                              f"— this is a credentials/quota problem, not a "
                              f"calendar one", file=sys.stderr)
                        stop.set()
                        break
                    continue

                recent.append(True)
                if df is None or df.empty:
                    # Half-day (15:45 is after a 13:00 close), or a chain the
                    # prefilter emptied. Recorded so a rerun does not re-request it.
                    options_db.record(con, d, root, "empty", detail=note)
                    empty += 1
                    print(f"{head} no quotes ({note})")
                    continue

                check_scale(df, d)
                options_db.write_partition(
                    con, df.reindex(columns=options_db.COLUMNS), d, root
                )
                written = options_db.write_quotes(con, df)
                options_db.record(con, d, root, "ok", contracts=written)
                ok += 1
                print(f"{head} {note}")

            if stop.is_set():
                for f in pending:
                    f.cancel()
                break

    elapsed = time.perf_counter() - t0
    print(f"{root}: {ok} ok, {empty} empty, {failed} failed in "
          f"{elapsed / 60:.1f} min ({elapsed / max(ok + empty + failed, 1):.1f}s "
          f"per session)")
    return not stop.is_set()


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
    p.add_argument("--workers", type=int, default=DEFAULT_WORKERS,
                   help=f"concurrent session fetches (default {DEFAULT_WORKERS})")
    args = p.parse_args()

    key = os.environ.get("DATABENTO_API_KEY")
    if not key:
        print("DATABENTO_API_KEY is not set", file=sys.stderr)
        return 2

    roots = args.root or ["SPY"]
    start = dt.date.fromisoformat(args.start)
    end = dt.date.fromisoformat(args.end) if args.end else start
    dates = [d.date() for d in pd.bdate_range(start, end)]

    if args.preview_cost:
        for root in roots:
            preview_cost(client_for(key), root, start)
        return 0

    con = options_db.connect()
    try:
        budget = {"spent": 0.0, "cap": args.max_cost}
        complete = True
        for root in roots:
            if not run(key, con, root, dates, args.force, budget, args.workers):
                complete = False
                break
        print(f"estimated spend this run: ${budget['spent']:.6f}")
        out = options_db.export(con)
        print(f"exported {out}: {options_db.summary(con)}")
    finally:
        con.close()
    return 0 if complete else 1


if __name__ == "__main__":
    raise SystemExit(main())
