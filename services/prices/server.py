"""Price/FX service for ptf-engine.

yfinance is the data source; DuckDB is the cache/store; Parquet is the
interchange the Rust engine reads (no C++ DuckDB in Rust). DuckDB + yfinance
live only here.

Run:  uv run uvicorn server:app --port 8001
Endpoints:
  POST /ensure {"symbols": ["AAPL", ...], "period": "2y"}
  GET  /health
"""

from __future__ import annotations

import datetime as dt
import os
from pathlib import Path

import duckdb
import pandas as pd
import yfinance as yf
from fastapi import FastAPI
from pydantic import BaseModel

DATA = Path(os.environ.get("PRICES_DATA") or (Path(__file__).resolve().parent / "data"))
DATA.mkdir(parents=True, exist_ok=True)
DB_PATH = DATA / "prices.duckdb"
PRICES_PARQUET = DATA / "prices.parquet"
FX_PARQUET = DATA / "fx.parquet"

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
    con.execute(
        """CREATE TABLE IF NOT EXISTS fx(
               ccy VARCHAR PRIMARY KEY, usd_per_unit DOUBLE, asof_date DATE)"""
    )
    return con


class EnsureReq(BaseModel):
    symbols: list[str] = []
    period: str = "2y"
    force: bool = False


def _is_fresh(con, symbol: str) -> bool:
    row = con.execute(
        "SELECT max(date) FROM prices WHERE symbol = ?", [symbol]
    ).fetchone()
    last = row[0] if row else None
    return last is not None and (dt.date.today() - last).days <= 3


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
    con.executemany("INSERT OR REPLACE INTO prices VALUES (?, ?, ?)", recs)

    row = con.execute(
        "SELECT count(*), max(date) FROM prices WHERE symbol = ?", [symbol]
    ).fetchone()
    return {"rows": row[0], "last": str(row[1]), "cached": False}


def fetch_fx(con) -> dict:
    today = dt.date.today()
    out = {}
    for ccy, spec in FX_SYMBOLS.items():
        if spec is None:
            con.execute("INSERT OR REPLACE INTO fx VALUES ('USD', 1.0, ?)", [today])
            out["USD"] = 1.0
            continue
        sym, invert = spec
        try:
            h = yf.Ticker(sym).history(period="5d", interval="1d", auto_adjust=True)
            last = float(h["Close"].dropna().iloc[-1])
            usd_per = (1.0 / last) if invert else last
            con.execute(
                "INSERT OR REPLACE INTO fx VALUES (?, ?, ?)", [ccy, usd_per, today]
            )
            out[ccy] = round(usd_per, 6)
        except Exception as e:  # noqa: BLE001
            out[ccy] = f"error: {e}"
    return out


def export(con) -> None:
    con.execute(
        f"""COPY (SELECT symbol, strftime(date, '%Y-%m-%d') AS date, close
                  FROM prices ORDER BY symbol, date)
            TO '{PRICES_PARQUET}' (FORMAT PARQUET)"""
    )
    con.execute(
        f"COPY (SELECT ccy, usd_per_unit FROM fx) TO '{FX_PARQUET}' (FORMAT PARQUET)"
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
        fx = fetch_fx(con)
        export(con)
    finally:
        con.close()
    return {"symbols": symbols, "fx": fx}
