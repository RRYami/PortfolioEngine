# ptf_engine — Agent Context

## Project
Portfolio analytics engine — domain layer in Rust.
Current focus: v1 fold and valuation complete. Next: persistence or API layer.

## Tech Stack
- **Language**: Rust stable, edition 2024
- **Async runtime**: tokio (reserved for future I/O layers)
- **Decimals**: `rust_decimal` — NEVER use `f64` for monetary amounts
- **Dates**: `chrono` (`NaiveDate` for trade/settle dates)
- **IDs**: `uuid` v4, wrapped in newtype structs
- **Errors**: `thiserror` for domain errors; `anyhow` only at application edges (not used yet)
- **Testing**: built-in `#[cfg(test)]` + `proptest` for property-based tests
- **Persistence (future)**: `sqlx` with Postgres, no ORM
- **Serialization (future)**: `serde`

## Build & Test
```bash
# Compile
cd backend && cargo build

# Run all tests (unit + integration + property)
cd backend && cargo test

# Run only property tests
cd backend && cargo test --test fold_properties
cd backend && cargo test --test valuation_properties

# Check formatting and lints
cd backend && cargo fmt --check
cd backend && cargo clippy -- -D warnings
```

## Architecture Conventions

### Layered architecture (domain → repositories → services → API)
- **Domain layer** (`backend/src/`) must remain dependency-free: no `sqlx`, no HTTP, no file I/O.
- I/O boundaries are traits: `PriceProvider`, `FxRateProvider`, `PortfolioRepository`, etc.
- Concrete impls live outside the domain.

### Type safety
- Newtype everything with units or identity: `InstrumentId`, `PortfolioId`, `LotId`, `Currency`, etc.
- `Money` is a struct of `(Decimal amount, Currency)`. Never sum or compare across currencies in the domain layer.
- `Currency` is a strict 3-byte ASCII-uppercase newtype. Use `Currency::USD`, `Currency::EUR`, etc.

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

### Errors
- `DomainError` is a typed enum with validation and fold variants.
- `FxError` has `RateUnavailable`, `ProviderError`, and `DivisionByZero` (for zero-rate inversion).
- `PriceError` has `PriceUnavailable` and `ProviderError`.
- `ValuationError` wraps `FxError` and `PriceError`, plus `PriceCurrencyMismatch`.
- Constructors on `TransactionKind` and `CorporateAction` validate invariants and return `Result<_, DomainError>`.

## Testing Conventions
- **Unit tests**: inline `#[cfg(test)]` modules per file for type-internal invariants.
- **Integration tests**: `tests/` directory for end-to-end and property-based tests.
- **Property tests**: `tests/fold_properties.rs` (11 properties) and `tests/valuation_properties.rs` (5 properties) using `proptest`. Default 256 cases per property.
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

## Deferred (do not implement unprompted)
- Persistence (Postgres schema, migrations)
- HTTP/API layer
- Frontend
- Authentication / multi-tenancy
- Borrow fees and margin interest accounting
- Options, futures, derivatives
- Snapshot caching for performance

## File Layout
```
backend/
  src/
    currency.rs          # Currency newtype, strict validation
    error.rs             # DomainError enum
    fold.rs              # fold() and apply() — core lot-closing logic
    fx.rs                # FxRateProvider trait, FxError, StaticFxRateProvider, TriangulatingFxProvider
    ids.rs               # Uuid newtypes (InstrumentId, LotId, etc.)
    instrument.rs        # Instrument, InstrumentKind
    lot.rs               # Lot struct with sequence, side, basis
    lot_method.rs        # LotMethod, LotSide, LotSelection, LotSelectionEntry
    money.rs             # Money { amount, currency }
    portfolio_config.rs  # PortfolioConfig { lot_method, base_currency }
    portfolio_state.rs   # PortfolioState { positions, cash, realized_pnl, next_lot_sequence }
    position.rs          # Position { instrument, currency, lots, realized_pnl }
    price.rs             # PriceProvider trait, PriceError, StaticPriceProvider
    transaction.rs       # Transaction, TransactionKind, CorporateAction + constructors
    valuation.rs         # ValuationError, PortfolioState::total_value()
  tests/
    fold_properties.rs       # proptest invariants for fold (11 properties)
    valuation_properties.rs  # proptest invariants for FX and valuation (5 properties)

frontend/      # Next.js app (to be scaffolded)
shared/        # API schema contract (ts-rs output or OpenAPI spec)
```

## Test Counts
- **141 unit tests** (inline `#[cfg(test)]` across all source files)
- **11 fold property tests** (`tests/fold_properties.rs`)
- **5 valuation property tests** (`tests/valuation_properties.rs`)
- **Total: 157 tests, all passing**

## How to Extend
1. Add new error variants to `DomainError` if needed.
2. Add new `TransactionKind` or `CorporateAction` variants with constructor validation.
3. Add a new arm in `apply()` in `fold.rs`.
4. Add unit tests in `fold.rs` and property tests in `tests/fold_properties.rs`.
5. For new provider traits (e.g. `CorporateActionProvider`), follow the pattern in `fx.rs` and `price.rs`:
   - Define the trait with a sync method returning `Result<_, TypedError>`.
   - Provide a `Static*` in-memory impl for tests.
   - Add property tests in `tests/valuation_properties.rs` or a new file.
6. Update this file if conventions or deferred items change.