"""Remove price rows dated on days the US market was closed.

Synthetic demo prices leaked into the real `prices` table on a business-day
calendar, which skips weekends but not market holidays. The result was a row
for NVDA on Thanksgiving priced 31% below the previous close -- a fabricated
session that more than doubled its estimated volatility (114% annualised
against a true 45%) and, because the following day's return is computed off
that close, injected a +36% day into the risk factor panel.

The reference calendar is taken from a symbol that is known-clean rather than
hardcoded: a date is a trading day if the reference traded on it. Rows outside
the reference's own span cannot be judged and are left alone.

Run with --apply to write; the default is a dry run.
"""

from __future__ import annotations

import argparse
import sys

import duckdb


DEFAULT_DB = "data/prices.duckdb"
DEFAULT_REFERENCE = "SPY"


def candidates(con: duckdb.DuckDBPyConnection, reference: str):
    """Rows inside the reference's span dated on a non-trading day."""
    return con.execute(
        """
        WITH cal AS (SELECT DISTINCT date FROM prices WHERE symbol = ?),
             span AS (SELECT min(date) mn, max(date) mx FROM prices WHERE symbol = ?)
        SELECT p.symbol, p.date, p.close
        FROM prices p, span
        WHERE p.date BETWEEN span.mn AND span.mx
          AND p.date NOT IN (SELECT date FROM cal)
        ORDER BY p.symbol, p.date
        """,
        [reference, reference],
    ).fetchall()


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=DEFAULT_DB)
    ap.add_argument(
        "--reference",
        default=DEFAULT_REFERENCE,
        help="symbol whose calendar defines a trading day",
    )
    ap.add_argument("--apply", action="store_true", help="delete (default: dry run)")
    args = ap.parse_args()

    con = duckdb.connect(args.db, read_only=not args.apply)

    ref_rows = con.execute(
        "SELECT count(*) FROM prices WHERE symbol = ?", [args.reference]
    ).fetchone()[0]
    if ref_rows < 200:
        print(
            f"reference {args.reference} has only {ref_rows} rows — too thin to "
            "define a calendar; pass --reference with a fuller series",
            file=sys.stderr,
        )
        return 1

    rows = candidates(con, args.reference)
    if not rows:
        print("no rows on non-trading dates")
        return 0

    print(f"{len(rows)} row(s) dated on days {args.reference} did not trade:")
    for symbol, date, close in rows:
        print(f"  {symbol:<6} {date}  {close:>12.4f}")

    # Rows the reference cannot vouch for either way.
    outside = con.execute(
        """
        WITH span AS (SELECT min(date) mn, max(date) mx FROM prices WHERE symbol = ?)
        SELECT p.symbol, count(*) FROM prices p, span
        WHERE p.date < span.mn OR p.date > span.mx
        GROUP BY 1 ORDER BY 1
        """,
        [args.reference],
    ).fetchall()
    for symbol, n in outside:
        print(f"note: {symbol} has {n} row(s) outside {args.reference}'s span — not checked")

    if not args.apply:
        print("\ndry run — pass --apply to delete")
        return 0

    # One transaction: a partial delete would leave the table in a state no
    # rerun could distinguish from a clean one.
    con.execute("BEGIN TRANSACTION")
    try:
        con.execute(
            """
            WITH cal AS (SELECT DISTINCT date FROM prices WHERE symbol = ?),
                 span AS (SELECT min(date) mn, max(date) mx FROM prices WHERE symbol = ?)
            DELETE FROM prices
            WHERE (symbol, date) IN (
                SELECT p.symbol, p.date FROM prices p, span
                WHERE p.date BETWEEN span.mn AND span.mx
                  AND p.date NOT IN (SELECT date FROM cal)
            )
            """,
            [args.reference, args.reference],
        )
    except Exception:
        con.execute("ROLLBACK")
        raise
    con.execute("COMMIT")
    print(f"\ndeleted {len(rows)} row(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
