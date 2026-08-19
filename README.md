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
containers share the `pricedata` volume — `prices` writes the Parquet snapshot,
`api` reads it; they also talk over the network for ensure-on-add. Portfolios,
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
- **Historical price provider**: `HistoricalPriceProvider` trait for lookback windows; `StaticHistoricalPriceProvider` for tests and demos.
- **Web app**: an Axum analytics API (`crates/api/`) over the engine, a Python price service (yfinance → DuckDB → Parquet), and a Next.js dashboard — see [Run the full stack (Docker)](#run-the-full-stack-docker).

## Status

**v1 fold and valuation complete. Analytics API + web dashboard implemented.**

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
- **Analytics API** (`crates/api/`): Axum HTTP service over the engine — create portfolios, add holdings, fetch a positions view (holdings valued at spot with their tax lots) and a risk payload (VaR/ES, component VaR, positions, and chart series) computed by `compute_var`
- **Price service + dashboard**: Python `services/prices/` (yfinance → DuckDB → Parquet) feeding the API, and a Next.js `frontend/` dashboard; the whole stack runs via Docker Compose
- 210 unit tests (including 28 Postgres repository tests and 9 auth tests) + 16 property tests + 36 serde round-trip tests, all passing

Deferred:
- Snapshot caching for performance
- Borrow fees, margin interest, derivatives

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
        instrument.rs      # Instrument, InstrumentKind
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
        serde_roundtrip.rs       # serde round-trip tests (35 tests, serde feature)
    api/                  # Axum HTTP API (ptf-api)
      Dockerfile
      src/
        main.rs            # server bootstrap + price-source selection (synthetic / parquet)
        handlers.rs        # routes: portfolios, holdings, positions, risk
        auth.rs            # session auth: register/login/logout/me, axum-login backend
        risk_view.rs       # maps VaRReport + PortfolioState → dashboard JSON
        positions_view.rs  # lightweight positions + tax-lot view (no Monte-Carlo)
        charts.rs          # P&L distribution, drawdown, historical-VaR series
        price_source.rs    # SyntheticPriceSource + ParquetPriceSource (reads Parquet)
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
- **No `f64` for money**: all monetary amounts use `rust_decimal::Decimal`. No floating-point rounding errors.
- **Sync traits, async adapters**: `FxRateProvider` and `PriceProvider` are synchronous in the domain. Real-world async fetching happens at the adapter layer — batch-fetch rates into a `StaticFxRateProvider`, then pass it to `total_value()`.
- **No silent substitution**: `total_value()` returns `ValuationError::PriceCurrencyMismatch` when a price's currency doesn't match the position's currency, `FxError::RateUnavailable` when a cross-currency rate is missing, and `PriceError::PriceUnavailable` when a price is missing. Zero and wrong-currency values are never substituted silently.

## License

MIT (or specify your preferred license)
