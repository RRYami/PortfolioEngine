# ptf_engine — Agent Context

## Project
Portfolio analytics engine — domain layer in Rust.
Current focus: transaction-to-position fold with lot accounting, multi-currency cash, and property-based tests.

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
cargo build

# Run all tests (unit + integration + property)
cargo test

# Run only property tests
cargo test --test fold_properties

# Check formatting and lints
cargo fmt --check
cargo clippy -- -D warnings
```

## Architecture Conventions

### Layered architecture (domain → repositories → services → API)
- **Domain layer** (`src/`) must remain dependency-free: no `sqlx`, no HTTP, no file I/O.
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

### Errors
- `DomainError` is a typed enum with validation and fold variants.
- Constructors on `TransactionKind` and `CorporateAction` validate invariants and return `Result<_, DomainError>`.

## Testing Conventions
- **Unit tests**: inline `#[cfg(test)]` modules per file for type-internal invariants.
- **Integration tests**: `tests/` directory for end-to-end and property-based tests.
- **Property tests**: `tests/fold_properties.rs` using `proptest`. Default 256 cases per property.
- When a property fails, trust proptest's shrinking — the minimized counterexample usually points straight at the bug.

## Key Design Decisions
1. Simultaneous long+short on the same instrument is **disallowed in v1** via fold-logic netting, but `Position` already supports it (single `Vec<Lot>` with `LotSide`). Lifting the restriction later requires no type changes.
2. Corporate actions live under `TransactionKind::CorporateAction(CorporateAction)`. Only `Split` and `ReverseSplit` are implemented; others are placeholders.
3. Dividends on short positions error with `DomainError::DividendOnShortPosition` in v1. Correct short-dividend semantics deferred.
4. Fees are baked into `basis_per_unit` at lot creation (pro-rata). Cash deduction uses gross + fees separately.
5. Chronological ordering of transactions is enforced by `fold`: strictly non-decreasing `trade_date`, error on violation.

## Deferred (do not implement unprompted)
- Persistence (Postgres schema, migrations)
- HTTP/API layer
- Frontend
- Authentication / multi-tenancy
- Borrow fees and margin interest accounting
- Options, futures, derivatives
- Snapshot caching for performance
- FX rate provider and base-currency valuation
- Price provider and mark-to-market

## File Layout
```
src/
  currency.rs          # Currency newtype, strict validation
  error.rs             # DomainError enum
  ids.rs               # Uuid newtypes (InstrumentId, LotId, etc.)
  instrument.rs        # Instrument, InstrumentKind
  lot.rs               # Lot struct with sequence, side, basis
  lot_method.rs        # LotMethod, LotSide, LotSelection, LotSelectionEntry
  money.rs             # Money { amount, currency }
  portfolio_config.rs  # PortfolioConfig { lot_method, base_currency }
  portfolio_state.rs   # PortfolioState { positions, cash, realized_pnl, next_lot_sequence }
  position.rs          # Position { instrument, currency, lots, realized_pnl }
  transaction.rs       # Transaction, TransactionKind, CorporateAction + constructors
  fold.rs              # fold() and apply() — core lot-closing logic

tests/
  fold_properties.rs   # proptest invariants
```

## How to Extend
1. Add new error variants to `DomainError` if needed.
2. Add new `TransactionKind` or `CorporateAction` variants with constructor validation.
3. Add a new arm in `apply()` in `fold.rs`.
4. Add unit tests in `fold.rs` and property tests in `tests/fold_properties.rs`.
5. Update this file if conventions or deferred items change.
