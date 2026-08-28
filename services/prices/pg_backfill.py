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
VENUE = "XNYS"

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
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
