# ptf_engine

A Rust domain-layer engine for portfolio analytics. Transactions are the immutable source of truth; positions, cash balances, and realized PnL are derived by folding the transaction history.

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
- **TUI Demo**: `cargo run -p ptf-tui` launches a `ratatui` read-only stakeholder demo with pre-seeded portfolios, a time-machine replay, lot-level inspector, currency exposure bars, live cross-currency valuation, and a VaR analytics screen.

## Status

**v1 fold and valuation complete. TUI demo implemented.**

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
- **TUI binary** (`crates/tui/`): keyboard-driven demo for stakeholders — portfolio picker, dashboard with positions & cash, transaction ledger, full-screen lot inspector, time-machine replay, currency exposure bar chart, cross-currency valuation popup, and VaR analytics screen
- 161 unit tests + 16 property tests + 35 serde round-trip tests, all passing

Deferred:
- Postgres persistence
- HTTP/API layer
- Snapshot caching for performance
- Borrow fees, margin interest, derivatives

## Quick start

```bash
# Build
cargo build --workspace

# Run the TUI demo
cargo run -p ptf-tui

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
  docker-compose.yml      # postgres:16-alpine on port 5433
  Makefile                # db-up, db-down, db-reset, test, etc.
  .env                    # DATABASE_URL for local dev
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
    tui/                  # TUI demo binary (ptf-tui)
      Cargo.toml
      src/
        main.rs            # crossterm event loop, screen state machine, popups, VaR analytics
        data.rs            # pre-seeded portfolios, instruments, transactions, prices, FX, historical prices
    persistence/          # Postgres implementations (coming)
  frontend/             # Next.js app (to be scaffolded)
  shared/               # API schema contract (OpenAPI spec)
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
