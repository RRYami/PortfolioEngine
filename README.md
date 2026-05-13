# ptf_engine

A Rust domain-layer engine for portfolio analytics. Transactions are the immutable source of truth; positions, cash balances, and realized PnL are derived by folding the transaction history.

## What it does

- **Transaction-to-position fold**: given a chronologically ordered list of transactions (deposits, withdrawals, buys, sells, dividends, splits, fees), compute the resulting portfolio state — positions, per-currency cash, and realized PnL.
- **Lot accounting**: every buy opens a `Lot`. Sells close lots using FIFO, LIFO, or user-specified `LotSelection::Specific`. Short positions are first-class: selling past your long quantity opens a short lot; buying past your short quantity covers it.
- **Multi-currency cash**: cash is tracked per-currency, never summed across currencies in the domain layer. Base-currency reporting is deferred to a future FX valuation layer.
- **Corporate actions**: splits and reverse-splits scale lot quantities and basis, preserving total cost basis.
- **Property-based tests**: nine proptest invariants guard the fold against regressions (cash conservation, basis preservation, determinism, currency isolation, and more).

## Status

**v1 fold is feature-complete.**

Implemented:
- Deposit, Withdrawal, Fee
- Buy / Sell with FIFO, LIFO, and `LotSelection::Specific`
- Short-side flips (sell-past-long → short; buy-past-short → long)
- Split, ReverseSplit
- Dividend (long positions only; short positions error in v1)
- Atomicity: validation happens before any state mutation
- 110 unit tests + 11 property tests, all passing

Deferred:
- FX rate provider and base-currency valuation
- Price provider and mark-to-market
- Persistence (Postgres)
- HTTP/API layer
- Snapshot caching for performance
- Borrow fees, margin interest, derivatives

## Quick start

```bash
# Build
cargo build

# Run all tests
cargo test

# Run only property tests
cargo test --test fold_properties

# Check formatting and lints
cargo fmt --check
cargo clippy -- -D warnings
```

## Architecture

```
src/
  fold.rs              # fold(&[Transaction], &PortfolioConfig) -> Result<PortfolioState, DomainError>
  transaction.rs       # Transaction, TransactionKind, CorporateAction (with constructor validation)
  lot.rs               # Lot with side, quantity, basis_per_unit, sequence (deterministic ordering)
  position.rs          # Position: instrument, currency, lots, realized_pnl
  portfolio_state.rs   # PortfolioState: positions, cash, realized_pnl, next_lot_sequence
  portfolio_config.rs  # PortfolioConfig: lot_method, base_currency
  money.rs             # Money { amount: Decimal, currency: Currency }
  currency.rs          # Currency newtype (3-letter ASCII uppercase)
  error.rs             # DomainError enum
  ids.rs               # Uuid newtypes (InstrumentId, LotId, etc.)
  instrument.rs        # Instrument, InstrumentKind
  lot_method.rs        # LotMethod, LotSide, LotSelection, LotSelectionEntry
```

The domain layer (`src/`) has **zero I/O dependencies** — no `sqlx`, no HTTP, no file I/O. I/O boundaries are defined as traits (`PriceProvider`, `FxRateProvider`, `PortfolioRepository`) with concrete implementations living outside the domain.

## Design highlights

- **Immutable transactions, derived state**: positions are never mutated directly. The fold is the canonical computation, making the system fully auditable and time-travel capable.
- **Atomic apply**: every transaction is validated before any state mutation. A failed transaction leaves `PortfolioState` unchanged.
- **Deterministic lot ordering**: `Lot::sequence` (a monotonic `u64` from `PortfolioState::next_lot_sequence`) guarantees that FIFO/LIFO selection is identical across runs, even when multiple lots share the same `open_date`.
- **No `f64` for money**: all monetary amounts use `rust_decimal::Decimal`. No floating-point rounding errors.

## License

MIT (or specify your preferred license)
