# ptf_engine — Agent Context

## Project
Portfolio analytics engine — domain layer in Rust.
Current focus: **TUI demo implemented.** Postgres persistence crate is next.

## Tech Stack
- **Language**: Rust stable, edition 2024
- **Async runtime**: tokio (used in repository implementations)
- **Decimals**: `rust_decimal` — NEVER use `f64` for monetary amounts
- **Dates**: `chrono` (`NaiveDate` for trade/settle dates)
- **IDs**: `uuid` v4, wrapped in newtype structs
- **Errors**: `thiserror` for domain errors; `anyhow` only at application edges (not used yet)
- **Testing**: built-in `#[cfg(test)]` + `proptest` for property-based tests
- **Serialization**: `serde` behind optional feature flag; `rust_decimal/serde-with-str` for string-formatted Decimals
- **Persistence**: `sqlx` with Postgres, no ORM (coming)
- **Async traits**: `async-trait` for repository contracts
- **TUI**: `ratatui` + `crossterm` for the demo binary (`crates/tui/`)

## Build & Test
```bash
# Compile workspace
cargo build --workspace

# Run the TUI demo
cargo run -p ptf-tui

# Run all tests (domain only)
cargo test --workspace

# Run all tests with all features (serde + in-memory repo + TUI)
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
- **Postgres implementations** will live in `crates/persistence/` (coming).
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
- Postgres schema and migrations
- HTTP/API layer
- Web frontend (Next.js)
- Authentication / multi-tenancy
- Borrow fees and margin interest accounting
- Options, futures, derivatives
- Snapshot caching for performance

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
        portfolio.rs       # Portfolio metadata (id, name, base_currency, lot_method)
        portfolio_config.rs# PortfolioConfig { lot_method, base_currency }
        portfolio_state.rs # PortfolioState { positions, cash, realized_pnl, next_lot_sequence }
        position.rs        # Position { instrument, currency, lots, realized_pnl }
        price.rs           # PriceProvider trait, PriceError, StaticPriceProvider
        repository/        # storage contracts and in-memory impls
        risk.rs            # MonteCarloConfig, VaRReport, AssetRisk, compute_var()
          mod.rs
          error.rs         # RepoError
          portfolio.rs     # PortfolioRepository trait
          transaction.rs   # TransactionRepository trait
          instrument.rs    # InstrumentRepository trait
          memory.rs        # InMemory*Repository impls
        transaction.rs     # Transaction, TransactionKind, CorporateAction + constructors
        valuation.rs       # ValuationError, PortfolioState::total_value()
      tests/
        fold_properties.rs       # proptest invariants for fold (11 properties)
        valuation_properties.rs  # proptest invariants for FX and valuation (5 properties)
        serde_roundtrip.rs       # serde round-trip tests (35 tests, serde feature)

    tui/                 # TUI demo binary (ptf-tui)
      Cargo.toml
      src/
        main.rs            # crossterm event loop, screen state machine, popups, VaR analytics screen
        data.rs            # pre-seeded portfolios, instruments, transactions, prices, FX rates, historical prices
    persistence/         # Postgres implementations (coming)
  frontend/            # Next.js app (to be scaffolded)
  shared/              # API schema contract (OpenAPI spec)
```

## Test Counts
- **161 unit tests** (inline `#[cfg(test)]` across all source files, including 16 repository memory tests and 4 risk tests)
- **11 fold property tests** (`tests/fold_properties.rs`)
- **5 valuation property tests** (`tests/valuation_properties.rs`)
- **35 serde round-trip tests** (`tests/serde_roundtrip.rs`, `serde` feature)
- **Total: 212 tests with all features, all passing**

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
   - Add tests in `repository/memory.rs` under `#[cfg(test)]`.
8. For TUI changes, follow the pattern in `crates/tui/src/main.rs`:
   - Keep all domain logic in `ptf-engine`; the TUI is pure presentation.
   - Pre-seed data in `crates/tui/src/data.rs` using `fold()` to derive `PortfolioState`.
   - Use `StaticPriceProvider`, `StaticFxRateProvider`, and `StaticHistoricalPriceProvider` for mock data.
   - Each screen is a `fn render_*` + a `fn handle_*_keys` pair.
   - Run `cargo clippy -p ptf-tui --all-features -- -D warnings` before committing.
9. Update this file if conventions or deferred items change.
