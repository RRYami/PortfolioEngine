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

## Status

**v1 fold and valuation complete.**

Implemented:
- Deposit, Withdrawal, Fee
- Buy / Sell with FIFO, LIFO, and `LotSelection::Specific`
- Short-side flips (sell-past-long → short; buy-past-short → long)
- Split, ReverseSplit
- Dividend (long positions only; short positions error in v1)
- Atomicity: validation happens before any state mutation
- `FxRateProvider` trait, `FxError`, `StaticFxRateProvider`, `TriangulatingFxProvider`
- `PriceProvider` trait, `PriceError`, `StaticPriceProvider`
- `ValuationError` (wraps `FxError`, `PriceError`, `PriceCurrencyMismatch`)
- `PortfolioState::total_value()` — multi-currency valuation with FX conversion
- 141 unit tests + 16 property tests, all passing

Deferred:
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
cargo test --test valuation_properties

# Check formatting and lints
cargo fmt --check
cargo clippy -- -D warnings
```

## Architecture

```
src/
  fold.rs              # fold(&[Transaction], &PortfolioConfig) -> Result<PortfolioState, DomainError>
  fx.rs                # FxRateProvider trait, FxError, StaticFxRateProvider, TriangulatingFxProvider
  price.rs             # PriceProvider trait, PriceError, StaticPriceProvider
  valuation.rs         # ValuationError, PortfolioState::total_value()
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

tests/
  fold_properties.rs       # proptest invariants for fold (11 properties)
  valuation_properties.rs # proptest invariants for FX and valuation (5 properties)
```

The domain layer (`src/`) has **zero I/O dependencies** — no `sqlx`, no HTTP, no file I/O. I/O boundaries are defined as traits (`PriceProvider`, `FxRateProvider`, `PortfolioRepository`) with concrete implementations living outside the domain.

## Design highlights

- **Immutable transactions, derived state**: positions are never mutated directly. The fold is the canonical computation, making the system fully auditable and time-travel capable.
- **Atomic apply**: every transaction is validated before any state mutation. A failed transaction leaves `PortfolioState` unchanged.
- **Deterministic lot ordering**: `Lot::sequence` (a monotonic `u64` from `PortfolioState::next_lot_sequence`) guarantees that FIFO/LIFO selection is identical across runs, even when multiple lots share the same `open_date`.
- **No `f64` for money**: all monetary amounts use `rust_decimal::Decimal`. No floating-point rounding errors.
- **Sync traits, async adapters**: `FxRateProvider` and `PriceProvider` are synchronous in the domain. Real-world async fetching happens at the adapter layer — batch-fetch rates into a `StaticFxRateProvider`, then pass it to `total_value()`.
- **No silent substitution**: `total_value()` returns `ValuationError::PriceCurrencyMismatch` when a price's currency doesn't match the position's currency, `FxError::RateUnavailable` when a cross-currency rate is missing, and `PriceError::PriceUnavailable` when a price is missing. Zero and wrong-currency values are never substituted silently.

## License

MIT (or specify your preferred license)