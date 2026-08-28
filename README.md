# ptf_engine

A Rust domain-layer engine for portfolio analytics. Transactions are the immutable source of truth; positions, cash balances, and realized PnL are derived by folding the transaction history.

## Run the full stack (Docker)

The web app — Rust analytics API, Python price service (yfinance → DuckDB →
Parquet), Next.js dashboard, and Postgres (TimescaleDB) — is orchestrated with
Docker Compose:

```bash
make up      # build + start everything, dashboard on http://localhost:3000
make logs    # tail logs
make down    # stop
```

Services (see `docker-compose.yml`): `frontend` (:3000) → `api` (:8080) →
`prices` (:8001), plus `postgres` (:5433, TimescaleDB). The `api` and `prices`
containers bind-mount `services/prices/data` — `prices` writes the Parquet
snapshot, `api` reads it; they also talk over the network for ensure-on-add. A
bind mount rather than a named volume because the option pipeline runs on the
host: the Databento ingest and `ptf-surface` both write there, and a managed
volume would put their output somewhere no container could reach. Portfolios,
instruments, transactions, users, and sessions are stored durably in Postgres
(the `pgdata` volume); the API applies embedded migrations on boot. The app
requires an account: register in the UI (first user or anyone, unless
`PTF_DISABLE_REGISTRATION=1`), then create portfolios and add holdings (real
prices are fetched live on first use). The dashboard has three pages, switched
from the sidebar: **Positions** (a sortable/filterable/paginated holdings table
with per-lot drill-in), **Risk** (VaR/ES, component VaR, and the chart series),
and **Performance** (risk-adjusted ratios vs a benchmark).

For frontend development with hot reload, run the backend in Docker
(`docker compose up -d prices api`) and the dashboard locally
(`cd frontend && PTF_API_URL=http://localhost:8080 npm run dev`).

## What it does

- **Transaction-to-position fold**: given a chronologically ordered list of transactions (deposits, withdrawals, buys, sells, dividends, splits, fees), compute the resulting portfolio state — positions, per-currency cash, and realized PnL.
- **Lot accounting**: every buy opens a `Lot`. Sells close lots using FIFO, LIFO, or user-specified `LotSelection::Specific`. Short positions are first-class: selling past your long quantity opens a short lot; buying past your short quantity covers it.
- **Multi-currency cash**: cash is tracked per-currency, never summed across currencies in the domain layer.
- **FX rate provider**: synchronous `FxRateProvider` trait with same-currency identity (returns `Decimal::ONE`), `StaticFxRateProvider` for in-memory tests, and `TriangulatingFxProvider` that resolves rates via direct → inverse → pivot triangulation (4 leg combinations).
- **Price provider**: synchronous `PriceProvider` trait returning `Money` so currency travels with the price.
- **Portfolio valuation**: `PortfolioState::total_value(fx, price, base, as_of)` sums cash (FX-converted) and position market values. Short positions naturally subtract. Missing prices, missing rates, and currency mismatches are loud failures — no silent substitution.
- **Corporate actions**: splits and reverse-splits scale lot quantities and basis, preserving total cost basis.
- **Property-based tests**: 16 proptest invariants guard fold, FX, and valuation against regressions.
- **Risk analytics (VaR/CVaR)**: Monte-Carlo `compute_var()` with Cholesky decomposition, configurable confidence levels and horizons, and per-asset component-VaR decomposition.
- **Listed options**: contracts are held as first-class instruments and revalued through a fitted volatility surface on every simulated path, rather than shocked linearly like a stock. See [Options and the volatility surface](#options-and-the-volatility-surface).
- **Historical price provider**: `HistoricalPriceProvider` trait for lookback windows; `StaticHistoricalPriceProvider` for tests and demos.
- **Web app**: an Axum analytics API (`crates/api/`) over the engine, a Python price service (yfinance → DuckDB → Parquet), and a Next.js dashboard — see [Run the full stack (Docker)](#run-the-full-stack-docker).

## Options and the volatility surface

An option cannot be risked like a stock. Its own price history is not a usable
return series — moneyness and time to expiry move underneath it — and a linear
price shock throws away the convexity that is most of the reason to hold one.
It will happily project a long call's value below zero and report a loss larger
than the premium.

So options are driven by their **underlying**, and revalued through a
volatility surface fitted offline. The pipeline runs in five stages, each
producing a table the next one reads:

| Stage | Produces | What it does |
|---|---|---|
| Ingest (`services/prices/ingest_databento.py`) | `market.option_quote` | One 15:45 ET chain snapshot per session from Databento OPRA, with quote sizes and a staleness marker |
| Forwards | `vol.forward_curve` | Recovers `F` and `DF` per expiry by put-call parity — no external rate curve or dividend estimate |
| Smiles | `vol.svi_slice` | One SVI slice per expiry, so the smile can be read *between* listed strikes |
| Grid | `vol.grid_cell` | Resamples onto fixed standardised-moneyness and constant-maturity axes, so a cell means the same thing on consecutive days |
| Factors | `vol.pca_loading`, `vol.pca_score` | Three principal components of daily log-vol changes: level, skew rotation, term-structure interaction |

Stages 2-5 are one command:

```bash
DATABASE_URL=... cargo run --release -p ptf-surface
```

Every artifact belongs to a **run**. `ptf-surface` writes one under a fresh
`run_id` and promotes it in the same transaction, so a build is invisible until
it is complete; the API reads through `vol.current_run`, and rolling back a bad
fit is an update to one row. Without `DATABASE_URL` the same command writes the
six Parquet files instead and the API falls back to reading them, which is the
no-database path — it reads the chain from
`services/prices/data/archive/latest/`, the verified archive, rather than from a
separate export.

At risk time the engine loads only the small artifacts it needs — forwards and
factors, about 4,300 rows against the 469,000 quotes they were built from — and
folds volatility factors into the *same* covariance matrix as spot returns.
That joint estimation is what preserves the leverage effect: model spot and vol
independently and a protective put stops hedging.

Holding one:

```bash
curl -X POST localhost:8080/api/portfolios/$PID/options \
  -H 'content-type: application/json' \
  -d '{"underlying":"SOXX","right":"call","strike":"540","expiry":"2026-11-20",
       "contracts":"2","premium":"28","currency":"USD"}'
```

The underlying is fetched from the price service on the way, since the risk run
needs its history. Contracts are stored under their OCC symbol
(`SOXX  261120C00540000`) — the same format the ingest writes — so a position
in the book matches a row in the chain without a translation table.

Writing options is refused rather than half-supported: opening a short needs a
margin model this book does not have. An option whose underlying has no fitted
surface is refused at the point of adding it, because accepting it would leave
a book whose `/risk` call fails outright, and silently under-reporting risk is
the worse failure.

**Known limitation.** Put-call parity holds as an identity only for European
exercise. SOXX, SPY and single names are American, so the extracted forward and
inverted volatility carry a small early-exercise bias. The style is recorded on
every contract; de-Americanising the prices is not yet done.

## Status

**v1 fold and valuation complete. Analytics API + web dashboard implemented.
Listed options are modelled end to end, from chain ingest to risk.**

Implemented:
- Deposit, Withdrawal, Fee
- Buy / Sell with FIFO, LIFO, and `LotSelection::Specific`
- Short-side flips (sell-past-long → short; buy-past-short → long)
- Split, ReverseSplit
- Dividend (long positions only; short positions error in v1)
- Atomicity: validation happens before any state mutation
- `FxRateProvider` trait, `FxError`, `StaticFxRateProvider`, `TriangulatingFxProvider`
- `PriceProvider` trait, `PriceError`, `StaticPriceProvider`
- `HistoricalPriceProvider` trait, `StaticHistoricalPriceProvider`
- `ValuationError` (wraps `FxError`, `PriceError`, `PriceCurrencyMismatch`)
- `PortfolioState::total_value()` — multi-currency valuation with FX conversion
- **Risk analytics**: Monte-Carlo VaR / CVaR with Cholesky-correlated sampling, configurable confidence levels / horizons / lookback, and per-asset component-VaR decomposition
- **serde feature**: optional `Serialize`/`Deserialize` on all domain types for JSON persistence and API serialization
- **Repository traits**: async `PortfolioRepository`, `TransactionRepository`, `InstrumentRepository` with thread-safe in-memory implementations
- **Users + authentication**: argon2id password hashing, server-side sessions (Postgres-backed, HttpOnly cookie), per-user portfolio ownership with 404-on-foreign isolation, login/register/logout endpoints, per-IP rate limiting on auth routes, and a login/register UI in the dashboard. Registration can be disabled with `PTF_DISABLE_REGISTRATION=1`.
- **Postgres persistence crate** (`crates/persistence/`): `sqlx`-based `Pg*Repository` implementations of the four repository traits, embedded migrations, and connection pool helpers. `transactions` is a TimescaleDB hypertable partitioned by `trade_date`; portfolios, instruments, and users stay plain reference tables.
- **Analytics API** (`crates/api/`): Axum HTTP service over the engine — create portfolios, add holdings and options, fetch a positions view (holdings valued at spot with their tax lots) and a risk payload (VaR/ES, component VaR, positions, and chart series) computed by `compute_var`
- **Option pricing kernel** (`vol.rs`): Black-76 on the forward, greeks, and an implied-vol inversion that reports its own conditioning — a premium that is nearly all intrinsic cannot pin a volatility down, so the solver refuses rather than returning whichever member of the admissible band it landed on
- **Volatility surface pipeline** (`crates/surface/`): parity forwards, SVI smiles, a constant-maturity grid, and a three-factor PCA of daily surface changes — see [Options and the volatility surface](#options-and-the-volatility-surface)
- **Options in risk**: `InstrumentKind::EquityOption` carries the full contract terms; `compute_var` drives each option off its underlying and reprices it through the shocked surface on every path, so a long option's loss is bounded by its premium the way a real one is
- **Price service + dashboard**: Python `services/prices/` (yfinance → DuckDB → Parquet) feeding the API, and a Next.js `frontend/` dashboard; the whole stack runs via Docker Compose
- 333 tests, all passing: 227 engine unit tests, 18 API tests, 28 Postgres repository tests, 22 property tests (fold, valuation, and the option kernel), 37 serde round-trip tests, and one that prices a real SOXX chain end to end

Deferred:
- Snapshot caching for performance
- Borrow fees, margin interest
- Writing options (needs a margin model) and closing them other than through `/sell`
- De-Americanising option prices before the parity fit
- Backtesting the VaR model (Kupiec, Christoffersen) — needs a second year of option history for an out-of-sample window

## Quick start

```bash
# Build
cargo build --workspace

# Run the analytics API (in-memory storage + synthetic prices;
# set DATABASE_URL for Postgres and PTF_PRICES=parquet for the price service)
cargo run -p ptf-api
DATABASE_URL=postgres://ptf:ptf@localhost:5433/ptf_engine cargo run -p ptf-api

# Auth env flags: PTF_DISABLE_REGISTRATION=1 (close sign-up),
# PTF_SECURE_COOKIES=1 (Secure cookie flag — enable behind TLS)
# Options: PTF_SURFACES=<dir> where the surface artifacts live (default
# services/prices/data, resolved against the working directory — set it
# explicitly if the API runs from anywhere else)

# Ingest option chains (costs money; --preview-cost prices a day first)
export DATABENTO_API_KEY=...   # or: set -a; source .env.local; set +a
uv run --extra options python services/prices/ingest_databento.py \
  --root SOXX --start 2025-08-21 --end 2026-08-20 --max-cost 10

# Build the surface artifacts from the ingested chains
cargo run --release -p ptf-surface

# Run all tests (domain only, no serde, no in-memory repo)
cargo test --workspace

# Run all tests with serde and in-memory repo enabled
cargo test --workspace --all-features

# Run only property tests
cargo test --workspace --test fold_properties
cargo test --workspace --test valuation_properties

# Check formatting and lints
cargo fmt --check
cargo clippy --workspace --all-features -- -D warnings

# Dev services (Postgres)
make db-up
make db-reset
make psql
```

## Workspace layout

```
ptf_engine/
  Cargo.toml              # workspace root
  Cargo.lock              # workspace lockfile
  docker-compose.yml      # TimescaleDB (timescale/timescaledb:2.17.2-pg16) on port 5433
  Makefile                # db-up, db-down, db-reset, test, etc.
  .env.example            # template; cp .env.example .env for local dev
  .env                    # DATABASE_URL for local dev (gitignored)
  .env.local              # local secrets, e.g. DATABENTO_API_KEY (gitignored)
  crates/
    engine/               # domain crate (ptf-engine)
      src/
        lib.rs             # public re-exports
        fold.rs            # fold() and apply() — core lot-closing logic
        fx.rs              # FxRateProvider, FxError, StaticFxRateProvider, TriangulatingFxProvider
        historical_price.rs # HistoricalPriceProvider, StaticHistoricalPriceProvider
        price.rs           # PriceProvider, PriceError, StaticPriceProvider
        risk.rs            # MonteCarloConfig, VaRReport, AssetRisk, compute_var()
        vol.rs             # Black-76 pricing, greeks, implied-vol inversion
        forward.rs         # put-call parity forwards + a session-wide discount curve
        svi.rs             # SVI smile parameterisation and calibration
        grid.rs            # resampling smiles onto constant-maturity, standardised-moneyness cells
        pca.rs             # one-sided Jacobi SVD, standardisation, factor reconstruction
        surface.rs         # SurfaceSnapshot + VolSurfaceProvider — what the risk engine prices against
        valuation.rs       # ValuationError, PortfolioState::total_value()
        transaction.rs     # Transaction, TransactionKind, CorporateAction + constructors
        lot.rs             # Lot struct with sequence, side, basis
        position.rs        # Position: instrument, currency, lots
        portfolio_state.rs # PortfolioState: positions, cash, realized_pnl
        portfolio.rs       # Portfolio: metadata (id, name, base_currency, lot_method)
        portfolio_config.rs# PortfolioConfig { lot_method, base_currency }
        money.rs           # Money { amount: Decimal, currency: Currency }
        currency.rs        # Currency newtype (3-letter ASCII uppercase)
        error.rs           # DomainError enum
        ids.rs             # Uuid newtypes (InstrumentId, LotId, etc.)
        instrument.rs      # Instrument, InstrumentKind (Equity, EquityOption), ExerciseStyle
        lot_method.rs      # LotMethod, LotSide, LotSelection, LotSelectionEntry
        repository/        # storage contracts and in-memory impls
          mod.rs           # re-exports
          error.rs         # RepoError
          portfolio.rs     # PortfolioRepository trait
          transaction.rs   # TransactionRepository trait
          instrument.rs    # InstrumentRepository trait
          memory.rs        # InMemory*Repository impls
      tests/
        fold_properties.rs       # proptest invariants for fold (11 properties)
        valuation_properties.rs  # proptest invariants for FX and valuation (5 properties)
        vol_properties.rs        # proptest invariants for the option kernel (6 properties)
        vol_real_chain.rs        # inverts 22 real SOXX quotes and checks the smile shape
        serde_roundtrip.rs       # serde round-trip tests (37 tests, serde feature)
    api/                  # Axum HTTP API (ptf-api)
      Dockerfile
      src/
        main.rs            # server bootstrap + price-source selection (synthetic / parquet)
        handlers.rs        # routes: portfolios, holdings, options, positions, risk
        auth.rs            # session auth: register/login/logout/me, axum-login backend
        risk_view.rs       # maps VaRReport + PortfolioState → dashboard JSON
        positions_view.rs  # lightweight positions + tax-lot view (no Monte-Carlo)
        charts.rs          # P&L distribution, drawdown, historical-VaR series
        price_source.rs    # SyntheticPriceSource + ParquetPriceSource (reads Parquet)
        surface_source.rs  # loads the surface artifacts into a VolSurfaceProvider
    surface/              # offline surface builder (ptf-surface)
      src/
        main.rs            # CLI: option quotes -> forwards, smiles, grid, factors
        quotes.rs          # reads the ingested chain
        build.rs           # per-session forwards, IV cloud, SVI slices, grid sampling
        factors.rs         # assembles the PCA panel and fits it per root
        write.rs           # the four output artifacts (a published column contract)
        error.rs           # SurfaceError
    persistence/          # Postgres persistence (ptf-persistence)
      Cargo.toml
      src/
        lib.rs             # connection pool helpers, embedded migrations
        error.rs           # sqlx → RepoError mapping (23505/23503)
        portfolio.rs       # PgPortfolioRepository
        transaction.rs     # PgTransactionRepository (hypertable)
        instrument.rs      # PgInstrumentRepository
        user.rs            # PgUserRepository
        test_util.rs       # DB-test pool helper (skips when DB is down)
      migrations/
        0001_initial.sql   # schema + TimescaleDB hypertable
        0002_users.sql     # users table + portfolios.user_id FK
  services/
    prices/               # Python price/FX service (yfinance → DuckDB → Parquet)
      server.py            # FastAPI: /ensure fetches and caches prices + FX
      options_db.py        # DuckDB store for option chains (separate file from prices)
      ingest_databento.py  # concurrent OPRA chain snapshots -> DuckDB + Parquet
      data/                # generated artifacts (gitignored)
  frontend/               # Next.js dashboard — Positions + Risk pages with a shared sidebar shell
```

The domain layer (`crates/engine/src/`) has **zero I/O dependencies** — no `sqlx`, no HTTP, no file I/O. I/O boundaries are defined as traits (`PriceProvider`, `FxRateProvider`, `PortfolioRepository`, etc.) with concrete implementations living outside the domain.

## Cargo features

| Feature | Description |
|---------|-------------|
| `serde` | Enables `Serialize`/`Deserialize` on all domain types. Required for JSONB persistence and API serialization. |
| `in-memory-repo` | Exposes `InMemoryPortfolioRepository`, `InMemoryTransactionRepository`, `InMemoryInstrumentRepository`. Useful for testing and standalone analytics. |

## Design highlights

- **Immutable transactions, derived state**: positions are never mutated directly. The fold is the canonical computation, making the system fully auditable and time-travel capable.
- **Atomic apply**: every transaction is validated before any state mutation. A failed transaction leaves `PortfolioState` unchanged.
- **Deterministic lot ordering**: `Lot::sequence` (a monotonic `u64` from `PortfolioState::next_lot_sequence`) guarantees that FIFO/LIFO selection is identical across runs, even when multiple lots share the same `open_date`.
- **No `f64` for money, no `Decimal` for statistics**: monetary amounts and exact contract terms — strikes, multipliers — use `rust_decimal::Decimal`, so there are no floating-point rounding errors and `InstrumentKind` stays `Copy`, `Eq` and `Hash`. Volatility, covariance and factor scores are `f64`: they are measurements, the maths is transcendental, and a VaR run does millions of these. The conversion happens at the boundary, which is what `risk.rs` already did for covariance.
- **One pricing kernel**: the same Black-76 implementation inverts a quote when the surface is fitted and prices a position when risk is run. Two implementations of one formula drift apart in the last decimals and disagree much further in the greeks, so the surface and the valuation are built from a single source.
- **Refuse rather than guess**: `implied_vol` reports when a premium cannot determine a volatility (deep in- or out-of-the-money, where vega is negligible) instead of returning an arbitrary member of the admissible band; the grid omits a cell it would have to extrapolate a maturity for; a negative fitted rate is floored rather than propagated as a discount factor above one.
- **Sync traits, async adapters**: `FxRateProvider` and `PriceProvider` are synchronous in the domain. Real-world async fetching happens at the adapter layer — batch-fetch rates into a `StaticFxRateProvider`, then pass it to `total_value()`.
- **No silent substitution**: `total_value()` returns `ValuationError::PriceCurrencyMismatch` when a price's currency doesn't match the position's currency, `FxError::RateUnavailable` when a cross-currency rate is missing, and `PriceError::PriceUnavailable` when a price is missing. Zero and wrong-currency values are never substituted silently.

## License

MIT (or specify your preferred license)
