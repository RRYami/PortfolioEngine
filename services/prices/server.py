"""Price/FX service for ptf-engine.

yfinance is the data source; DuckDB is the cache/store; Parquet is the
interchange the Rust engine reads (no C++ DuckDB in Rust). DuckDB + yfinance
live only here.

Run:  uv run uvicorn server:app --port 8001
Endpoints:
  POST /ensure {"symbols": ["AAPL", ...], "period": "2y"}

FX is stored as a dated series, not a spot snapshot: converting a tax lot's
cost at its own trade-date rate (and valuing a historical equity curve) needs
a rate per day, not one rate for everything.
  GET  /health
"""

from __future__ import annotations

import datetime as dt
import logging
import os
from pathlib import Path

import duckdb
import pandas as pd
import yfinance as yf
from fastapi import FastAPI
from pydantic import BaseModel

log = logging.getLogger(__name__)

DATA = Path(os.environ.get("PRICES_DATA") or (Path(__file__).resolve().parent / "data"))
DATA.mkdir(parents=True, exist_ok=True)
DB_PATH = DATA / "prices.duckdb"
PRICES_PARQUET = DATA / "prices.parquet"
FX_PARQUET = DATA / "fx.parquet"

# How far back to pull FX. Deliberately longer than the default price window:
# a tax lot can be far older than the price history a risk chart needs, and its
# cost must be converted at *its* trade-date rate. Five daily series is a
# rounding error next to per-symbol price history, so the window is cheap.
FX_PERIOD = os.environ.get("FX_PERIOD", "10y")

# yfinance FX symbols → how to turn the quote into USD-per-unit.
FX_SYMBOLS = {
    "USD": None,
    "EUR": ("EURUSD=X", False),   # quote is USD per EUR
    "GBP": ("GBPUSD=X", False),   # quote is USD per GBP
    "JPY": ("USDJPY=X", True),    # quote is JPY per USD → invert
    "CHF": ("USDCHF=X", True),    # quote is CHF per USD → invert
}

app = FastAPI(title="ptf-prices")


def connect() -> duckdb.DuckDBPyConnection:
    con = duckdb.connect(str(DB_PATH))
    con.execute(
        """CREATE TABLE IF NOT EXISTS prices(
               symbol VARCHAR, date DATE, close DOUBLE,
               PRIMARY KEY (symbol, date))"""
    )
    # The fx table used to be a spot snapshot keyed by ccy alone. DuckDB cannot
    # add a column to a primary key in place, so an old cache is dropped and
    # refetched — cheap, and it keeps the shape unambiguous.
    cols = {
        r[0]
        for r in con.execute(
            "SELECT column_name FROM information_schema.columns "
            "WHERE table_name = 'fx'"
        ).fetchall()
    }
    if cols and "date" not in cols:
        con.execute("DROP TABLE fx")
    con.execute(
        """CREATE TABLE IF NOT EXISTS fx(
               ccy VARCHAR, date DATE, usd_per_unit DOUBLE,
               PRIMARY KEY (ccy, date))"""
    )
    return con


class EnsureReq(BaseModel):
    symbols: list[str] = []
    period: str = "2y"
    force: bool = False


def _upsert(con, table: str, df: pd.DataFrame, keys: tuple[str, ...]) -> None:
    """Replace-by-key in one set-based statement.

    DuckDB is columnar: a row-by-row `INSERT OR REPLACE` against a primary key
    probes the index per row and costs milliseconds *each* — ~8s for 2600 FX
    rows. Deleting the incoming keys and inserting the frame in bulk does the
    same job ~400x faster. `df` column order must match the table.
    """
    if df.empty:
        return
    key_expr = ", ".join(keys)
    con.register("_incoming", df)
    try:
        con.execute(
            f"DELETE FROM {table} WHERE ({key_expr}) "
            f"IN (SELECT {key_expr} FROM _incoming)"
        )
        con.execute(f"INSERT INTO {table} SELECT * FROM _incoming")
    finally:
        con.unregister("_incoming")


def _is_fresh(con, symbol: str) -> bool:
    row = con.execute(
        "SELECT max(date) FROM prices WHERE symbol = ?", [symbol]
    ).fetchone()
    last = row[0] if row else None
    return last is not None and (dt.date.today() - last).days <= 3


# A symbol needs at least this many peer sessions before its calendar is
# trusted to judge another symbol's dates. Below it, a thin or brand-new
# series would reject perfectly good rows.
_CALENDAR_QUORUM = 200

# Venue whose sessions define a trading day. Matches market.trading_day.
CALENDAR = "XNYS"


def _drop_non_trading_days(con, symbol: str, recs: list) -> tuple[list, list]:
    """Split `recs` into rows on known trading days and rows on closed days.

    A row dated on a day the market was shut is not a price. Synthetic demo
    prices once leaked in on a business-day calendar -- weekends skipped but
    not holidays -- and put NVDA on Thanksgiving 31% below the previous close.
    That single fabricated session more than doubled its estimated volatility
    and, since the next day's return is computed against it, injected a +36%
    day into the risk factor panel.

    The calendar is the union of dates from other symbols that already have a
    substantial history, so it needs no hardcoded holiday list and follows
    whatever exchange the book actually trades on. With no such peer yet,
    everything is accepted: a first symbol has nothing to be checked against.
    """
    peers = [
        r[0]
        for r in con.execute(
            "SELECT symbol FROM prices WHERE symbol <> ? "
            "GROUP BY symbol HAVING count(*) >= ?",
            [symbol, _CALENDAR_QUORUM],
        ).fetchall()
    ]
    if not peers:
        return recs, []

    marks = ",".join("?" * len(peers))
    rows = con.execute(
        f"SELECT DISTINCT date FROM prices WHERE symbol IN ({marks})", peers
    ).fetchall()
    calendar = {r[0] for r in rows}
    lo, hi = min(calendar), max(calendar)

    keep, dropped = [], []
    for rec in recs:
        date = rec[1]
        # Outside the peers' span there is no calendar to check against, so
        # the row is kept rather than guessed at.
        if lo <= date <= hi and date not in calendar:
            dropped.append(rec)
        else:
            keep.append(rec)
    return keep, dropped


def fetch_symbol(con, symbol: str, period: str, force: bool) -> dict:
    if not force and _is_fresh(con, symbol):
        row = con.execute(
            "SELECT count(*), max(date) FROM prices WHERE symbol = ?", [symbol]
        ).fetchone()
        return {"rows": row[0], "last": str(row[1]), "cached": True}

    hist = yf.Ticker(symbol).history(period=period, interval="1d", auto_adjust=True)
    if hist is None or hist.empty or "Close" not in hist:
        raise ValueError(f"no price data for {symbol}")

    recs = [
        (symbol, idx.date(), float(v))
        for idx, v in hist["Close"].items()
        if pd.notna(v)
    ]
    if not recs:
        raise ValueError(f"no usable closes for {symbol}")

    recs, dropped = _drop_non_trading_days(con, symbol, recs)
    if dropped:
        log.warning(
            "%s: dropped %d row(s) dated on non-trading days: %s",
            symbol,
            len(dropped),
            ", ".join(str(r[1]) for r in dropped[:5]),
        )
    if not recs:
        raise ValueError(f"no usable closes for {symbol} on trading days")

    _upsert(
        con,
        "prices",
        pd.DataFrame(recs, columns=["symbol", "date", "close"]),
        ("symbol", "date"),
    )

    row = con.execute(
        "SELECT count(*), max(date) FROM prices WHERE symbol = ?", [symbol]
    ).fetchone()
    return {"rows": row[0], "last": str(row[1]), "cached": False}


def _fresh_currencies(con) -> set[str]:
    """Currencies whose series already reaches (near) today.

    FX had no freshness check while prices did, so every /ensure refetched and
    rewrote a decade of rates — including the ensure-on-read the performance
    page used to do on every request.
    """
    today = dt.date.today()
    return {
        ccy
        for ccy, last in con.execute(
            "SELECT ccy, max(date) FROM fx GROUP BY ccy"
        ).fetchall()
        if last is not None and (today - last).days <= 3
    }


def fetch_fx(con, period: str = FX_PERIOD, force: bool = False) -> dict:
    """Refresh the dated USD-per-unit series for every supported currency."""
    out: dict[str, dict | str] = {}
    dates: set[dt.date] = set()
    fresh = set() if force else _fresh_currencies(con)

    for ccy, spec in FX_SYMBOLS.items():
        if spec is None:
            continue  # USD is filled in below, once the date span is known
        if ccy in fresh:
            out[ccy] = {"cached": True}
            continue
        sym, invert = spec
        try:
            h = yf.Ticker(sym).history(period=period, interval="1d", auto_adjust=True)
            closes = h["Close"].dropna()
            if closes.empty:
                out[ccy] = "error: no data"
                continue
            rows = []
            for ts, close in closes.items():
                close = float(close)
                if close == 0.0:
                    continue  # a zero quote would invert to infinity
                d = ts.date()
                rows.append((ccy, d, (1.0 / close) if invert else close))
                dates.add(d)
            _upsert(
                con,
                "fx",
                pd.DataFrame(rows, columns=["ccy", "date", "usd_per_unit"]),
                ("ccy", "date"),
            )
            out[ccy] = {
                "rows": len(rows),
                "first": str(rows[0][1]),
                "last": str(rows[-1][1]),
                "latest": round(rows[-1][2], 6),
            }
        except Exception as e:  # noqa: BLE001
            out[ccy] = f"error: {e}"

    # USD is the pivot: 1.0 on every date any other currency quotes, so a
    # USD-denominated lot never misses a rate the others have.
    if dates:
        _upsert(
            con,
            "fx",
            pd.DataFrame(
                [("USD", d, 1.0) for d in sorted(dates)],
                columns=["ccy", "date", "usd_per_unit"],
            ),
            ("ccy", "date"),
        )
        out["USD"] = {"rows": len(dates), "latest": 1.0}
    elif "USD" in fresh:
        out["USD"] = {"cached": True}
    return out


def sync_postgres(con) -> dict:
    """Mirror closes and rates into Postgres, if a database is configured.

    The API reads `market.equity_close`; without this a fetch would land in
    DuckDB and the served numbers would quietly stay stale until someone ran
    the backfill by hand.

    Best-effort by design. A price fetch that succeeded should not be reported
    as failed because the database is down, and DuckDB remains the record --
    pg_backfill.py reconciles whatever was missed.
    """
    dsn = os.environ.get("DATABASE_URL")
    if not dsn:
        return {"synced": False, "reason": "DATABASE_URL unset"}
    try:
        import exchange_calendars as xc
        import psycopg
    except ImportError as e:  # pragma: no cover - image without the extras
        return {"synced": False, "reason": f"missing dependency: {e.name}"}

    closes = con.execute("SELECT symbol, date, close FROM prices").fetchall()
    rates = con.execute("SELECT ccy, date, usd_per_unit FROM fx").fetchall()
    if not closes:
        return {"synced": False, "reason": "no rows"}

    # Extend the session calendar to cover what is being written. Sourced from
    # the exchange, never inferred from the prices themselves: a calendar
    # derived from observed dates would authorise exactly the bad rows the
    # foreign key exists to reject.
    lo, hi = min(r[1] for r in closes), max(r[1] for r in closes)
    cal = xc.get_calendar(CALENDAR)
    lo = max(lo, cal.first_session.date())
    hi = min(hi, cal.last_session.date())
    sessions = [(d.date(), CALENDAR) for d in cal.sessions_in_range(lo, hi)]

    try:
        with psycopg.connect(dsn, connect_timeout=5) as conn:
            with conn.cursor() as cur:
                cur.executemany(
                    "INSERT INTO market.trading_day (session_date, venue) "
                    "VALUES (%s, %s) ON CONFLICT (session_date) DO NOTHING",
                    sessions,
                )
                cur.executemany(
                    "INSERT INTO market.equity_close (symbol, session_date, close) "
                    "VALUES (%s, %s, %s) ON CONFLICT (symbol, session_date) "
                    "DO UPDATE SET close = EXCLUDED.close",
                    closes,
                )
                cur.executemany(
                    "INSERT INTO market.fx_rate (ccy, rate_date, usd_per_unit) "
                    "VALUES (%s, %s, %s) ON CONFLICT (ccy, rate_date) "
                    "DO UPDATE SET usd_per_unit = EXCLUDED.usd_per_unit",
                    rates,
                )
            conn.commit()
    except Exception as e:  # noqa: BLE001
        log.warning("postgres sync failed: %s", e)
        return {"synced": False, "reason": str(e)}
    return {"synced": True, "closes": len(closes), "fx": len(rates)}


def export(con) -> None:
    con.execute(
        f"""COPY (SELECT symbol, strftime(date, '%Y-%m-%d') AS date, close
                  FROM prices ORDER BY symbol, date)
            TO '{PRICES_PARQUET}' (FORMAT PARQUET)"""
    )
    con.execute(
        f"""COPY (SELECT ccy, strftime(date, '%Y-%m-%d') AS date, usd_per_unit
                  FROM fx ORDER BY ccy, date)
            TO '{FX_PARQUET}' (FORMAT PARQUET)"""
    )


@app.get("/health")
def health() -> dict:
    return {"status": "ok"}


@app.post("/ensure")
def ensure(req: EnsureReq) -> dict:
    con = connect()
    symbols: dict[str, dict] = {}
    try:
        for s in req.symbols:
            sym = s.strip().upper()
            try:
                symbols[sym] = fetch_symbol(con, sym, req.period, req.force)
            except Exception as e:  # noqa: BLE001
                symbols[sym] = {"error": str(e)}
        fx = fetch_fx(con, force=req.force)
        export(con)
        pg = sync_postgres(con)
    finally:
        con.close()
    return {"symbols": symbols, "fx": fx, "postgres": pg}
