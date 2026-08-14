# ptf_engine — Agent Context

## Project
Portfolio analytics engine — domain layer in Rust.
Current focus: **Users + authentication implemented** — argon2id passwords, server-side sessions (Postgres), per-user portfolio ownership, login/register UI. Next: snapshot caching / performance work.

## Tech Stack
- **Language**: Rust stable, edition 2024
- **Async runtime**: tokio (used in repository implementations)
- **Decimals**: `rust_decimal` — NEVER use `f64` for monetary amounts
- **Dates**: `chrono` (`NaiveDate` for trade/settle dates)
- **IDs**: `uuid` v4, wrapped in newtype structs
- **Errors**: `thiserror` for domain errors; `anyhow` only at application edges (not used yet)
- **Testing**: built-in `#[cfg(test)]` + `proptest` for property-based tests
- **Serialization**: `serde` behind optional feature flag; `rust_decimal/serde-with-str` for string-formatted Decimals
- **Persistence**: `sqlx` with Postgres + TimescaleDB (`timescale/timescaledb:2.17.2-pg16`), no ORM. Embedded migrations via `sqlx::migrate!()`. `sqlx-cli` for migration authoring. Runtime-checked queries (no `query!` macros, no `.sqlx` offline data).
- **Async traits**: `async-trait` for repository contracts
- **Web API**: `axum` + `tower-http` for the HTTP service (`crates/api/`); `arrow`/`parquet` to read the price snapshot
- **Auth**: `axum-login` 0.18 + `tower-sessions` 0.14 + `tower-sessions-sqlx-store` 0.15 (keep this version set in sync — both target tower-sessions 0.14). `password-auth` (argon2id) for hashing. `tower-governor` 0.8 for auth rate limiting (crate name is `tower_governor`).

## Build & Test
```bash
# Compile workspace
cargo build --workspace

# Run the analytics API (in-memory; DATABASE_URL switches to Postgres)
cargo run -p ptf-api

# Run all tests (Postgres tests skip unless `make db-up` was run)
cargo test --workspace

# Run all tests with all features (serde + in-memory repo)
cargo test --workspace --all-features

# Run only property tests
cargo test --workspace --test fold_properties
cargo test --workspace --test valuation_properties

# Check formatting and lints
cargo fmt --check
cargo clippy --workspace --all-features -- -D warnings
```

## Dev Services
```bash
make db-up      # start postgres:16-alpine on 5433
make db-down    # stop postgres
make db-reset   # destroy volume and restart
make psql       # connect with psql
make test       # cargo test --workspace
```

## Architecture Conventions

### Layered architecture (domain → repositories → services → API)
- **Domain layer** (`crates/engine/src/`) must remain dependency-free: no `sqlx`, no HTTP, no file I/O.
- **Repository traits** live in `crates/engine/src/repository/` (async, trait-only).
- **In-memory implementations** live in `crates/engine/src/repository/memory.rs`, gated by `#[cfg(any(test, feature = "in-memory-repo"))]`.
- **Postgres implementations** live in `crates/persistence/src/{portfolio,transaction,instrument}.rs` as `Pg*Repository` structs over a shared `sqlx::PgPool`. The API selects storage by env: `DATABASE_URL` set → Postgres (migrations applied on boot), unset → in-memory.
- I/O boundaries are traits: `PriceProvider`, `FxRateProvider`, `PortfolioRepository`, `TransactionRepository`, `InstrumentRepository`.

### Cargo features
- `serde`: optional. Derives `Serialize`/`Deserialize` on all domain types. Required for JSONB persistence and API responses.
- `in-memory-repo`: optional. Exposes `InMemory*Repository` types. Auto-enabled for `cfg(test)`.

### Type safety
- Newtype everything with units or identity: `InstrumentId`, `PortfolioId`, `LotId`, `Currency`, etc.
- `Money` is a struct of `(Decimal amount, Currency)`. Never sum or compare across currencies in the domain layer.
- `Currency` is a strict 3-byte ASCII-uppercase newtype. Use `Currency::USD`, `Currency::EUR`, etc.
- All ID types implement `Display` (delegates to inner UUID).

### Transactions are immutable source of truth
- Positions are **derived** by folding transactions; never mutated directly by callers.
- `fold(&[Transaction], &PortfolioConfig) -> Result<PortfolioState, DomainError>` is the canonical computation.

### Lot model
- Each `Lot` has a `side: LotSide` (`Long` or `Short`), always-positive `quantity`, `basis_per_unit`, `open_date`, and monotonic `sequence: u64` for deterministic ordering.
- Short positions are first-class: a sell past long quantity closes longs then opens a short lot; a buy past short quantity covers shorts then opens a long lot.
- `LotSelection::Specific` allows user-directed lot closing; `LotSelection::Method` overrides the portfolio default for one transaction.

### Cash model
- Cash is held per-currency in `HashMap<Currency, Decimal>`.
- Balances are **never summed across currencies** in the domain layer.
- Cash balances may go negative; overdraft detection is a caller concern.

### FX and valuation
- `FxRateProvider` is a synchronous trait with a default `rate()` method that returns `Decimal::ONE` for same-currency identity and delegates to `rate_impl()` for cross-currency lookups.
- `TriangulatingFxProvider<P>` wraps any `FxRateProvider` and attempts direct rate → inverse rate → triangulation via a pivot currency (4 leg-direction combinations). Inversion is subject to `Decimal` truncation; round-trip tests use an epsilon.
- `PriceProvider` returns `Money` so the currency travels with the price. `total_value()` checks that each price's currency matches the position's currency and returns `ValuationError::PriceCurrencyMismatch` on mismatch — no silent substitution.
- `PortfolioState::total_value()` is synchronous. Async fetching happens at the adapter layer; the domain stays a pure function.

### Repository traits
- All repository traits are `async` via `async-trait`, `Send + Sync`.
- `TransactionRepository::list` returns transactions in chronological order (by `trade_date`, tie-broken by insertion sequence).
- `InstrumentRepository::upsert` enforces symbol uniqueness: same symbol with different `InstrumentId` returns `RepoError::AlreadyExists`.
- Errors are explicit: `RepoError::NotFound`, `AlreadyExists`, `Conflict`, `Serialization`, `Database`.

### Postgres schema and `Pg*` implementations
- `transactions` is the only **TimescaleDB hypertable** (partitioned by `trade_date`, 1-month chunks) — the domain's append-only, time-ranged event log. `portfolios`/`instruments` are plain reference tables.
- Hypertable constraint: any unique index must include the partition column, so the PK is `(id, trade_date)`. Insertion order is preserved by a `seq BIGINT GENERATED ALWAYS AS IDENTITY` column; `list`/`list_until` use `ORDER BY trade_date, seq` (matches the memory impl's tiebreak).
- Enum-ish columns (`portfolios.lot_method`, `instruments.kind`) are JSONB and mapped with `sqlx::types::Json<T>` straight through serde — no parallel string-mapping code. `Currency` columns are `CHAR(3)` ↔ `Currency` via its validating `TryFrom<&str>`.
- Error mapping (`persistence/src/error.rs`): `RowNotFound` and FK violations (`23503`) → `NotFound`; unique violations (`23505`) → `AlreadyExists`. Note `PgTransactionRepository::append` is stricter than memory: appending to a missing portfolio errors (the FK enforces it).
- The `pgdata` volume is wiped when switching Postgres images (`make db-reset`) — TimescaleDB needs `shared_preload_libraries`, so a volume initialized by plain `postgres:16-alpine` cannot be reused.
- DB tests use a **fresh small pool per test** (`test_util::test_pool`), never a shared static pool: pooled connections created on a dropped tokio runtime become zombies (`PoolTimedOut`). Tests skip gracefully when the DB is unreachable.
- DB tests run against a dedicated **`ptf_engine_test`** database (auto-created on first run; override with `TEST_DATABASE_URL`) so fixtures never pollute the dev database or dashboard.

### Authentication and ownership
- **Users own portfolios**: `Portfolio.user_id` (engine `UserId` newtype) is set at creation from the session; `PortfolioRepository::list(user_id)` returns only the owner's rows. Ownership transfer is not supported (UPDATE never touches `user_id`).
- **Auth model**: server-side sessions via `axum-login` + `tower-sessions`. Session data lives in the `tower_sessions.session` Postgres table (store migrates itself on boot; `MemoryStore` when running without `DATABASE_URL`). Cookie: `ptf_session`, HttpOnly, SameSite=Lax, Secure via `PTF_SECURE_COOKIES=1`, 7-day idle expiry. `login` regenerates the session id; `logout` destroys the session server-side.
- **Passwords**: argon2id via `password-auth`; policy is NIST 800-63B length-only (8–128 chars). The engine `User` type deliberately has **no serde derive** — the API returns a `UserSummary` DTO; hashes never serialize.
- **Auth routes** (`crates/api/src/auth.rs`): `POST /api/auth/register` (auto-login; `PTF_DISABLE_REGISTRATION=1` → 403), `POST /api/auth/login`, `POST /api/auth/logout`, `GET /api/auth/me`. Login errors are generic ("invalid email or password") — no user enumeration.
- **Authorization**: the `SessionUser` extractor 401s unauthenticated requests; every portfolio-scoped handler loads via `owned_portfolio()` which returns **404** for missing *or* foreign-owned portfolios (no ID enumeration).
- **Rate limiting**: `tower-governor` per-IP (5/min) on `/api/auth/login` + `/api/auth/register` only. Requires `ConnectInfo` — `main.rs` serves `into_make_service_with_connect_info`; tests insert `ConnectInfo` into request extensions.
- **Email normalization**: `User::new` lowercases the email; uniqueness and `by_email` are case-insensitive.

### Serialization
- `serde` is an optional feature. When enabled:
  - `Currency` serializes as `"USD"` (custom impl, not derived from `[u8; 3]`)
  - `Decimal` serializes as string `"123.45"` (via `rust_decimal/serde-with-str`)
  - `NaiveDate` as ISO 8601, UUIDs as hyphenated strings
  - `TransactionKind` is internally tagged: `{"kind": "buy", ...}`
  - `CorporateAction` is internally tagged: `{"action": "split", ...}`
  - `LotSide`/`LotMethod` use `rename_all = "snake_case"`

### Errors
- `DomainError` is a typed enum with validation and fold variants.
- `FxError` has `RateUnavailable`, `ProviderError`, and `DivisionByZero` (for zero-rate inversion).
- `PriceError` has `PriceUnavailable` and `ProviderError`.
- `ValuationError` wraps `FxError` and `PriceError`, plus `PriceCurrencyMismatch`.
- `RepoError` has `NotFound`, `AlreadyExists`, `Conflict`, `Serialization`, `Database`.
- Constructors on `TransactionKind` and `CorporateAction` validate invariants and return `Result<_, DomainError>`.

## Testing Conventions
- **Unit tests**: inline `#[cfg(test)]` modules per file for type-internal invariants.
- **Postgres tests**: use `test_util::test_pool()` (per-test pool, skip when DB is down) against the `ptf_engine_test` database — never the dev `ptf_engine` database.
- **Integration tests**: `tests/` directory for end-to-end and property-based tests.
- **Property tests**: `tests/fold_properties.rs` (11 properties) and `tests/valuation_properties.rs` (5 properties) using `proptest`. Default 256 cases per property.
- **Serde tests**: `tests/serde_roundtrip.rs` (35 tests), compiled only with `serde` feature. Round-trip every domain type through JSON.
- When a property fails, trust proptest's shrinking — the minimized counterexample usually points straight at the bug.

## Key Design Decisions
1. Simultaneous long+short on the same instrument is **disallowed in v1** via fold-logic netting, but `Position` already supports it (single `Vec<Lot>` with `LotSide`). Lifting the restriction later requires no type changes.
2. Corporate actions live under `TransactionKind::CorporateAction(CorporateAction)`. Only `Split` and `ReverseSplit` are implemented; others are placeholders.
3. Dividends on short positions error with `DomainError::DividendOnShortPosition` in v1. Correct short-dividend semantics deferred.
4. Fees are baked into `basis_per_unit` at lot creation (pro-rata). Cash deduction uses gross + fees separately.
5. Chronological ordering of transactions is enforced by `fold`: strictly non-decreasing `trade_date`, error on violation.
6. FX/valuation traits are **synchronous** in the domain. Async fetching is an adapter concern: batch-fetch rates into a `StaticFxRateProvider`, then pass it to `total_value()`.
7. Same-currency FX requests return `Decimal::ONE` via the trait's default `rate()` method; implementors never handle this case.
8. `TriangulatingFxProvider` attempts rate resolution in order: direct → inverse → triangulation (4 leg combos). Inversion round-trips are approximate due to `Decimal` truncation.
9. `total_value()` returns `ValuationError::PriceCurrencyMismatch` when a price's currency doesn't match the position's currency. No silent substitution of zero or wrong-currency prices.
10. Repository traits are **async** (`async-trait`) because real storage is async, but the domain fold stays sync. The in-memory impl uses `tokio::sync::RwLock` to match the async interface.
11. `Portfolio` (metadata) and `PortfolioState` (derived) are separate types. Metadata is persisted; state is always re-derived from transactions.

## Deferred (do not implement unprompted)
- Email verification and password reset (needs SMTP)
- MFA/TOTP, passkeys, OAuth/OIDC
- Account deletion, portfolio sharing between users, admin roles
- Borrow fees and margin interest accounting
- Options, futures, derivatives
- Snapshot caching for performance
- TimescaleDB extras: compression, retention policies, continuous aggregates

## File Layout
```
ptf_engine/
  Cargo.toml              # workspace root
  Cargo.lock
  docker-compose.yml
  Makefile
  .env
  crates/
    engine/
      Cargo.toml
      src/
        lib.rs             # public re-exports
        currency.rs        # Currency newtype, strict validation
        error.rs           # DomainError enum
        fold.rs            # fold() and apply() — core lot-closing logic
        fx.rs              # FxRateProvider trait, FxError, StaticFxRateProvider, TriangulatingFxProvider
        historical_price.rs # HistoricalPriceProvider trait, StaticHistoricalPriceProvider
        ids.rs             # Uuid newtypes (InstrumentId, LotId, etc.)
        instrument.rs      # Instrument, InstrumentKind
        lot.rs             # Lot struct with sequence, side, basis
        lot_method.rs      # LotMethod, LotSide, LotSelection, LotSelectionEntry
        money.rs           # Money { amount, currency }
        portfolio.rs       # Portfolio metadata (id, user_id, name, base_currency, lot_method)
        portfolio_config.rs# PortfolioConfig { lot_method, base_currency }
        portfolio_state.rs # PortfolioState { positions, cash, realized_pnl, next_lot_sequence }
        position.rs        # Position { instrument, currency, lots, realized_pnl }
        price.rs           # PriceProvider trait, PriceError, StaticPriceProvider
        repository/        # storage contracts and in-memory impls
        risk.rs            # MonteCarloConfig, VaRReport, AssetRisk, compute_var()
        user.rs            # User (id, email, password_hash) — no serde (credential material)
          mod.rs
          error.rs         # RepoError
          portfolio.rs     # PortfolioRepository trait
          transaction.rs   # TransactionRepository trait
          instrument.rs    # InstrumentRepository trait
          user.rs          # UserRepository trait
          memory.rs        # InMemory*Repository impls
        transaction.rs     # Transaction, TransactionKind, CorporateAction + constructors
        valuation.rs       # ValuationError, PortfolioState::total_value()
      tests/
        fold_properties.rs       # proptest invariants for fold (11 properties)
        valuation_properties.rs  # proptest invariants for FX and valuation (5 properties)
        serde_roundtrip.rs       # serde round-trip tests (35 tests, serde feature)

    api/                 # Axum HTTP API (ptf-api)
      Dockerfile
      src/
        main.rs            # server bootstrap + price-source selection
        handlers.rs        # routes: portfolios, holdings, risk
        auth.rs            # axum-login backend, auth routes, SessionUser extractor
        risk_view.rs       # VaRReport + PortfolioState → dashboard JSON
        charts.rs          # P&L distribution, drawdown, historical-VaR series
        price_source.rs    # SyntheticPriceSource + ParquetPriceSource
    persistence/         # Postgres persistence implementations (ptf-persistence)
      Cargo.toml
      src/
        lib.rs            # connection pool helpers, embedded migrations
        error.rs          # sqlx → RepoError mapping (23505/23503)
        portfolio.rs      # PgPortfolioRepository
        transaction.rs    # PgTransactionRepository (hypertable)
        instrument.rs     # PgInstrumentRepository
        user.rs           # PgUserRepository
        test_util.rs      # DB-test pool helper (skips when DB is down)
      migrations/
        0001_initial.sql  # schema + TimescaleDB hypertable
        0002_users.sql    # users table + portfolios.user_id FK
  services/
    prices/              # Python price/FX service (yfinance → DuckDB → Parquet)
  frontend/              # Next.js dashboard (risk desk UI)
```

## Test Counts
- **167 engine unit tests** (inline `#[cfg(test)]` across all source files, including 21 repository memory tests and 4 risk tests)
- **28 persistence tests** (2 migration/hypertable smoke tests + 26 `Pg*Repository` tests; need `make db-up`, skip gracefully without it)
- **15 API tests** (2 auth validation unit tests + 7 auth stack integration tests via `tower::oneshot` + 6 others)
- **11 fold property tests** (`tests/fold_properties.rs`)
- **5 valuation property tests** (`tests/valuation_properties.rs`)
- **36 serde round-trip tests** (`tests/serde_roundtrip.rs`, `serde` feature)
- **Total: 262 tests with all features, all passing**

## How to Extend
1. Add new error variants to `DomainError` or `RepoError` if needed.
2. Add new `TransactionKind` or `CorporateAction` variants with constructor validation.
3. Add a new arm in `apply()` in `fold.rs`.
4. Add unit tests in `fold.rs` and property tests in `tests/fold_properties.rs`.
5. For new provider traits (e.g. `CorporateActionProvider`), follow the pattern in `fx.rs` and `price.rs`:
   - Define the trait with a sync method returning `Result<_, TypedError>`.
   - Provide a `Static*` in-memory impl for tests.
   - Add property tests in `tests/valuation_properties.rs` or a new file.
6. For new risk analytics (e.g. new `compute_*` functions), follow the pattern in `risk.rs`:
   - Add a `Config` struct with a sensible default constructor.
   - Return a typed `Report` struct with per-asset and portfolio-level slices.
   - Use `f64` only inside the statistical simulation; surface `Money` (Decimal) to callers.
   - Add unit tests for edge cases (empty portfolio, flat prices, zero covariance).
7. For new repository traits, follow the pattern in `repository/`:
   - Define the async trait in `repository/<name>.rs`.
   - Add an in-memory impl in `repository/memory.rs`.
   - Add a Postgres impl in `crates/persistence/src/<name>.rs` (migration + `Pg*` struct + error mapping via `map_sqlx`).
   - Add tests: memory tests under `#[cfg(test)]`; DB tests using `test_util::test_pool()` (skip when DB is down).
8. For API changes, follow the pattern in `crates/api/`:
   - Keep all domain logic in `ptf-engine`; the API only orchestrates and shapes JSON.
   - Add routes in `handlers.rs`; map engine output to the dashboard contract in `risk_view.rs`.
   - Market data goes through the `PriceSource` trait (`price_source.rs`) — never read a feed directly.
   - Run `cargo clippy -p ptf-api -- -D warnings` before committing.
9. Update this file if conventions or deferred items change.
