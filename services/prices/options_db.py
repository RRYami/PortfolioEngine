"""DuckDB store for option-chain snapshots.

Deliberately a **separate database file** from `prices.duckdb`. DuckDB allows a
single writer process, and `server.py` opens the price store on every /ensure;
a multi-hour options backfill holding that lock would fail every price refresh,
and with it add-holding and the performance page.

No network here on purpose — schema, upsert, resume and export are all testable
without spending money at Databento.
"""

from __future__ import annotations

import datetime as dt
import os
from pathlib import Path

import duckdb
import pandas as pd

DATA = Path(os.environ.get("PRICES_DATA") or (Path(__file__).resolve().parent / "data"))
DB_PATH = DATA / "options.duckdb"

# Venue whose sessions define a trading day, matching market.trading_day.
CALENDAR = "XNYS"
OPTIONS_PARQUET = DATA / "options.parquet"
RAW_DIR = DATA / "raw_quotes"

# Column contract for `options.parquet`. The Rust side will read this, so the
# names and order are a published interface — extend at the end, don't reorder.
COLUMNS = [
    "quote_date",
    "root",
    "expiry",
    "opt_right",
    "strike",
    "instrument_id",
    "raw_symbol",
    "bid",
    "ask",
    "mid",
    "rel_spread",
    "tte",
    "snapshot_ts",
    "bid_size",
    "ask_size",
    "last_update_ts",
]


def connect(path: Path | None = None) -> duckdb.DuckDBPyConnection:
    """Open (creating if needed) the options store with its schema applied."""
    target = path or DB_PATH
    target.parent.mkdir(parents=True, exist_ok=True)
    con = duckdb.connect(str(target))
    # Keyed on contract identity, not instrument_id: OPRA assigns those per
    # dataset-day and can recycle them, whereas (root, expiry, right, strike)
    # is what actually names the contract.
    con.execute(
        """CREATE TABLE IF NOT EXISTS option_quotes(
               quote_date    DATE    NOT NULL,
               root          VARCHAR NOT NULL,
               expiry        DATE    NOT NULL,
               opt_right     VARCHAR NOT NULL,
               strike        DOUBLE  NOT NULL,
               instrument_id BIGINT,
               raw_symbol    VARCHAR,
               bid           DOUBLE,
               ask           DOUBLE,
               mid           DOUBLE,
               rel_spread    DOUBLE,
               tte           DOUBLE,
               snapshot_ts   TIMESTAMPTZ,
               bid_size      INTEGER,
               ask_size      INTEGER,
               last_update_ts TIMESTAMPTZ,
               PRIMARY KEY (quote_date, root, expiry, opt_right, strike))"""
    )
    # Migration for stores created before quote sizes and staleness were kept.
    # `upsert` does `INSERT ... SELECT *`, which is positional, so these have to
    # land in the same order as the tail of COLUMNS.
    for name, kind in (
        ("bid_size", "INTEGER"),
        ("ask_size", "INTEGER"),
        ("last_update_ts", "TIMESTAMPTZ"),
    ):
        con.execute(f"ALTER TABLE option_quotes ADD COLUMN IF NOT EXISTS {name} {kind}")
    # Lets a restarted backfill skip days already paid for, and distinguishes a
    # session that genuinely had no chain from one that errored.
    con.execute(
        """CREATE TABLE IF NOT EXISTS option_ingest_log(
               quote_date  DATE    NOT NULL,
               root        VARCHAR NOT NULL,
               contracts   INTEGER,
               status      VARCHAR NOT NULL,  -- 'ok' | 'empty' | 'error'
               detail      VARCHAR,
               ingested_at TIMESTAMPTZ NOT NULL,
               PRIMARY KEY (quote_date, root))"""
    )
    return con


def upsert(con, table: str, df: pd.DataFrame, keys: tuple[str, ...]) -> None:
    """Replace-by-key in one set-based statement.

    Row-by-row `INSERT OR REPLACE` against a primary key probes the index per
    row in DuckDB (a columnar engine) and costs milliseconds *each* — measured
    at 8.3s for 2600 rows against 0.02s for this. An option chain is ~1500 rows
    per day, so the difference decides whether a year's backfill is minutes or
    hours. (`server.py` carries its own copy for the price store; kept separate
    so the deployed service has no new import.)
    """
    if df.empty:
        return
    key_expr = ", ".join(keys)
    con.register("_incoming", df)
    try:
        # Both statements or neither. The delete is unconditional but the
        # insert can still fail the primary key -- two instrument_ids parsing
        # to one (root, expiry, right, strike), which OPRA does produce on
        # adjusted contracts. Autocommitted separately, that leaves the delete
        # applied and the replacement rows gone: the day silently loses the
        # chain it already had. The caller records the raised error and the
        # date can be re-ingested.
        con.execute("BEGIN TRANSACTION")
        try:
            con.execute(
                f"DELETE FROM {table} WHERE ({key_expr}) "
                f"IN (SELECT {key_expr} FROM _incoming)"
            )
            con.execute(f"INSERT INTO {table} SELECT * FROM _incoming")
        except Exception:
            con.execute("ROLLBACK")
            raise
        con.execute("COMMIT")
    finally:
        con.unregister("_incoming")


def write_quotes(con, df: pd.DataFrame) -> int:
    """Upsert one day's chain. `df` must carry exactly [`COLUMNS`]."""
    if df.empty:
        return 0
    out = df.reindex(columns=COLUMNS)
    upsert(
        con,
        "option_quotes",
        out,
        ("quote_date", "root", "expiry", "opt_right", "strike"),
    )
    return len(out)


def record(con, date: dt.date, root: str, status: str, contracts: int = 0,
           detail: str | None = None) -> None:
    upsert(
        con,
        "option_ingest_log",
        pd.DataFrame(
            [(date, root, contracts, status, detail, dt.datetime.now(dt.timezone.utc))],
            columns=["quote_date", "root", "contracts", "status", "detail",
                     "ingested_at"],
        ),
        ("quote_date", "root"),
    )


def done_dates(con, root: str) -> set[dt.date]:
    """Dates already ingested for `root` — 'ok' or a confirmed-empty session.

    Errors are excluded so a rerun retries them; 'empty' is not, because a
    holiday will be empty every time and re-requesting it still costs.
    """
    return {
        r[0]
        for r in con.execute(
            "SELECT quote_date FROM option_ingest_log "
            "WHERE root = ? AND status IN ('ok', 'empty')",
            [root],
        ).fetchall()
    }


def write_partition(con, df: pd.DataFrame, date: dt.date, root: str) -> Path:
    """Raw immutable per-date parquet, hive-partitioned for direct scanning.

    Written through DuckDB rather than `DataFrame.to_parquet`, which would pull
    in pyarrow purely as a writer — DuckDB is already a hard dependency here.
    """
    part = RAW_DIR / f"date={date.isoformat()}"
    part.mkdir(parents=True, exist_ok=True)
    path = part / f"{root}.parquet"
    con.register("_partition", df)
    try:
        con.execute(f"COPY (SELECT * FROM _partition) TO '{path}' (FORMAT PARQUET)")
    finally:
        con.unregister("_partition")
    return path


def sync_postgres(con, dsn: str | None = None) -> dict:
    """Mirror the quote store and ingest log into Postgres.

    `ptf-surface` reads `market.option_quote` when DATABASE_URL is set, so a
    backfill that only reached DuckDB would leave the fitted surface stale.

    Best-effort, like the price sync: a Databento session that was paid for and
    stored should not be reported as failed because the database is down. DuckDB
    stays the record and pg_backfill.py reconciles.

    Only the log is allowed non-trading days -- recording that a session was a
    holiday is its purpose. Quotes must fall on a session, and the calendar is
    extended from the exchange rather than inferred from the data.
    """
    dsn = dsn or os.environ.get("DATABASE_URL")
    if not dsn:
        return {"synced": False, "reason": "DATABASE_URL unset"}
    try:
        import exchange_calendars as xc
        import psycopg
    except ImportError as e:  # pragma: no cover
        return {"synced": False, "reason": f"missing dependency: {e.name}"}

    cols = ", ".join(COLUMNS)
    quotes = con.execute(
        f"SELECT {cols} FROM option_quotes "
        "ORDER BY quote_date, root, expiry, opt_right, strike"
    ).fetchall()
    log = con.execute(
        "SELECT quote_date, root, contracts, status, detail, ingested_at "
        "FROM option_ingest_log ORDER BY quote_date, root"
    ).fetchall()
    if not quotes:
        return {"synced": False, "reason": "no quotes"}

    cal = xc.get_calendar(CALENDAR)
    lo = max(min(r[0] for r in quotes), cal.first_session.date())
    hi = min(max(r[0] for r in quotes), cal.last_session.date())
    sessions = [(d.date(), CALENDAR) for d in cal.sessions_in_range(lo, hi)]

    try:
        with psycopg.connect(dsn, connect_timeout=5) as conn:
            with conn.cursor() as cur:
                cur.executemany(
                    "INSERT INTO market.trading_day (session_date, venue) "
                    "VALUES (%s, %s) ON CONFLICT (session_date) DO NOTHING",
                    sessions,
                )
                # Replace wholesale rather than upserting row by row: the store
                # is the source of truth and two thirds of a million
                # round-tripped INSERTs is minutes, not seconds.
                cur.execute("TRUNCATE market.option_quote")
                with cur.copy(
                    f"COPY market.option_quote ({cols}) FROM STDIN"
                ) as copy:
                    for row in quotes:
                        copy.write_row(row)
                cur.execute("TRUNCATE market.option_ingest_log")
                with cur.copy(
                    "COPY market.option_ingest_log (quote_date, root, contracts, "
                    "status, detail, ingested_at) FROM STDIN"
                ) as copy:
                    for row in log:
                        copy.write_row(row)
            conn.commit()
    except Exception as e:  # noqa: BLE001
        return {"synced": False, "reason": str(e)}
    return {"synced": True, "quotes": len(quotes), "log": len(log)}


def export(con, path: Path | None = None) -> Path:
    """Consolidate the store into one parquet — the engine-facing artifact."""
    target = path or OPTIONS_PARQUET
    target.parent.mkdir(parents=True, exist_ok=True)
    cols = ", ".join(COLUMNS)
    con.execute(
        f"""COPY (SELECT {cols} FROM option_quotes
                  ORDER BY quote_date, root, expiry, opt_right, strike)
            TO '{target}' (FORMAT PARQUET)"""
    )
    return target


def summary(con) -> dict:
    row = con.execute(
        "SELECT count(*), count(DISTINCT quote_date), min(quote_date), "
        "max(quote_date) FROM option_quotes"
    ).fetchone()
    return {"rows": row[0], "days": row[1], "first": str(row[2]),
            "last": str(row[3])}
