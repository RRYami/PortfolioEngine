# ptf-prices

Price/FX service for `ptf-engine`. Downloads daily closes via **yfinance**,
caches them in **DuckDB**, and exports a **Parquet** snapshot that the Rust
engine reads (`ParquetPriceSource`). DuckDB and yfinance live only here — Rust
never links the C++ DuckDB library; it reads Parquet with pure-Rust crates.

## Run

```bash
cd services/prices
uv run uvicorn server:app --port 8001
```

## Endpoints

- `POST /ensure` — `{"symbols": ["AAPL","NVDA"], "period": "2y", "force": false}`
  Fetches missing/stale symbols, upserts into DuckDB, refreshes FX, and rewrites
  `data/prices.parquet` + `data/fx.parquet`. Idempotent (cached if data is < 3
  days old unless `force`).
- `GET /health`

## Data

- `data/prices.duckdb` — working store (`prices`, `fx` tables).
- `data/prices.parquet` — `(symbol, date, close)` consumed by the Rust API.
- `data/fx.parquet` — `(ccy, usd_per_unit)` for FX conversion to base currency.

The Rust API calls `/ensure` automatically when a new holding is added
(ensure-on-add), then reads the Parquet for risk computation.
