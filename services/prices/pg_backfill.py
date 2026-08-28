"""Seed the trading calendar and move equity closes and FX into Postgres.

Idempotent: safe to rerun. The calendar is upserted, the price tables use
ON CONFLICT DO UPDATE, and the row counts are checked against the source
afterwards.

The calendar comes from `exchange_calendars`, not from the price data. That
distinction is the whole reason `market.trading_day` exists: derived from
observed dates it would have contained all twelve of the market holidays that
carried fabricated NVDA and AAPL prices, and would have authorised precisely
the rows it is there to reject.

    python pg_backfill.py                    # dry run: report, change nothing
    python pg_backfill.py --apply
"""

from __future__ import annotations

import argparse
import datetime as dt
import os
import sys

import duckdb
import exchange_calendars as xc
import psycopg

DEFAULT_DSN = "postgres://ptf:ptf@localhost:5433/ptf_engine"
PRICES_DB = "data/prices.duckdb"
OPTIONS_DB = "data/options.duckdb"
VENUE = "XNYS"

# Columns of market.option_quote, in table order.
QUOTE_COLUMNS = [
    "quote_date", "root", "expiry", "opt_right", "strike",
    "instrument_id", "raw_symbol", "bid", "ask", "mid",
    "rel_spread", "tte", "snapshot_ts", "bid_size", "ask_size",
    "last_update_ts",
]

# Seed a year either side of the data so a later backfill or a fresh fetch does
# not immediately hit a missing session.
PAD = dt.timedelta(days=365)


def calendar_sessions(lo: dt.date, hi: dt.date) -> list[dt.date]:
    cal = xc.get_calendar(VENUE)
    lo = max(lo, cal.first_session.date())
    hi = min(hi, cal.last_session.date())
    return [d.date() for d in cal.sessions_in_range(lo, hi)]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--dsn", default=os.environ.get("DATABASE_URL", DEFAULT_DSN))
    ap.add_argument("--db", default=PRICES_DB)
    ap.add_argument("--options-db", default=OPTIONS_DB)
    ap.add_argument("--apply", action="store_true")
    args = ap.parse_args()

    duck = duckdb.connect(args.db, read_only=True)
    closes = duck.execute(
        "SELECT symbol, date, close FROM prices ORDER BY symbol, date"
    ).fetchall()
    fx = duck.execute(
        "SELECT ccy, date, usd_per_unit FROM fx ORDER BY ccy, date"
    ).fetchall()

    dates = [r[1] for r in closes]
    lo, hi = min(dates), max(dates)
    fx_lo = min(r[1] for r in fx)
    sessions = calendar_sessions(min(lo, fx_lo) - PAD, hi + PAD)

    print(f"source      : {len(closes):,} closes, {len(fx):,} fx rows")
    print(f"price window: {lo} .. {hi}")
    print(f"calendar    : {len(sessions):,} {VENUE} sessions "
          f"{sessions[0]} .. {sessions[-1]}")

    # Every close must land on a session or the foreign key rejects it. Report
    # before writing: a mismatch here means either bad data or the wrong venue,
    # and both are worth looking at rather than discovering mid-COPY.
    known = set(sessions)
    orphans = sorted({d for d in dates if d not in known})
    if orphans:
        print(f"\n{len(orphans)} close(s) not on a {VENUE} session — these would be "
              f"rejected:", file=sys.stderr)
        by_date = {}
        for sym, d, _ in closes:
            if d in known:
                continue
            by_date.setdefault(d, []).append(sym)
        for d in orphans[:20]:
            print(f"  {d}  {', '.join(sorted(by_date[d]))}", file=sys.stderr)
        print("\nrefusing to load: fix the data (see clean_prices.py) or the venue",
              file=sys.stderr)
        return 1
    print("all closes fall on a trading session")

    if not args.apply:
        print("\ndry run — pass --apply to write")
        return 0

    with psycopg.connect(args.dsn) as conn:
        with conn.cursor() as cur:
            cur.executemany(
                "INSERT INTO market.trading_day (session_date, venue) VALUES (%s, %s) "
                "ON CONFLICT (session_date) DO NOTHING",
                [(d, VENUE) for d in sessions],
            )
            cur.executemany(
                "INSERT INTO market.equity_close (symbol, session_date, close) "
                "VALUES (%s, %s, %s) "
                "ON CONFLICT (symbol, session_date) DO UPDATE SET close = EXCLUDED.close",
                closes,
            )
            cur.executemany(
                "INSERT INTO market.fx_rate (ccy, rate_date, usd_per_unit) "
                "VALUES (%s, %s, %s) "
                "ON CONFLICT (ccy, rate_date) DO UPDATE "
                "SET usd_per_unit = EXCLUDED.usd_per_unit",
                fx,
            )
        conn.commit()

        with conn.cursor() as cur:
            cur.execute("SELECT count(*) FROM market.trading_day")
            n_cal = cur.fetchone()[0]
            cur.execute("SELECT count(*) FROM market.equity_close")
            n_eq = cur.fetchone()[0]
            cur.execute("SELECT count(*) FROM market.fx_rate")
            n_fx = cur.fetchone()[0]

    print(f"\nloaded: {n_cal:,} sessions, {n_eq:,} closes, {n_fx:,} fx rows")
    ok = n_eq == len(closes) and n_fx == len(fx)
    print("counts match source" if ok else "COUNT MISMATCH")

    if os.path.exists(args.options_db):
        ok = load_options(args.dsn, args.options_db, known) and ok

    return 0 if ok else 1


def load_options(dsn: str, db: str, sessions: set[dt.date]) -> bool:
    """Copy option quotes and the ingest log.

    COPY rather than executemany: two thirds of a million rows through
    round-tripped INSERTs takes minutes, and there is no upsert to do on a
    first load.
    """
    duck = duckdb.connect(db, read_only=True)
    cols = ", ".join(QUOTE_COLUMNS)
    quotes = duck.execute(f"SELECT {cols} FROM option_quotes").fetchall()
    log = duck.execute(
        "SELECT quote_date, root, contracts, status, detail, ingested_at "
        "FROM option_ingest_log"
    ).fetchall()

    orphans = sorted({r[0] for r in quotes} - sessions)
    if orphans:
        print(f"\n{len(orphans)} option session(s) are not {VENUE} sessions: "
              f"{orphans[:5]}", file=sys.stderr)
        return False

    # The log deliberately carries non-trading days -- recording that a session
    # was a holiday is its purpose -- so it has no foreign key and needs no
    # check here.
    holidays = sum(1 for r in log if r[0] not in sessions)

    print(f"\noptions: {len(quotes):,} quotes, {len(log)} log rows "
          f"({holidays} on non-sessions, as expected)")

    with psycopg.connect(dsn) as conn:
        with conn.cursor() as cur:
            cur.execute("TRUNCATE market.option_quote")
            with cur.copy(
                f"COPY market.option_quote ({cols}) FROM STDIN"
            ) as copy:
                for row in quotes:
                    copy.write_row(row)
            cur.execute("TRUNCATE market.option_ingest_log")
            with cur.copy(
                "COPY market.option_ingest_log "
                "(quote_date, root, contracts, status, detail, ingested_at) "
                "FROM STDIN"
            ) as copy:
                for row in log:
                    copy.write_row(row)
        conn.commit()
        with conn.cursor() as cur:
            cur.execute("SELECT count(*) FROM market.option_quote")
            n_q = cur.fetchone()[0]
            cur.execute("SELECT count(*) FROM market.option_ingest_log")
            n_l = cur.fetchone()[0]

    ok = n_q == len(quotes) and n_l == len(log)
    print(f"loaded: {n_q:,} quotes, {n_l} log rows — "
          + ("counts match source" if ok else "COUNT MISMATCH"))
    return ok


if __name__ == "__main__":
    raise SystemExit(main())
