use std::collections::HashMap;

use rust_decimal::Decimal;

use crate::currency::Currency;
use crate::ids::InstrumentId;
use crate::position::Position;

/// The derived state of a portfolio after folding its transaction history.
///
/// `positions` maps each instrument to its [`Position`] (a collection of lots).
/// `cash` holds per-currency balances; they are **never** summed across
/// currencies in the domain layer.
/// `realized_pnl` tracks closed-lot profits/losses per currency.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PortfolioState {
    pub(crate) positions: HashMap<InstrumentId, Position>,
    pub(crate) cash: HashMap<Currency, Decimal>,
    pub(crate) realized_pnl: HashMap<Currency, Decimal>,
    /// Monotonic counter for lot sequence numbers. Ensures deterministic
    /// ordering of lots created on the same date.
    pub(crate) next_lot_sequence: u64,
}

impl Default for PortfolioState {
    fn default() -> Self {
        Self::new()
    }
}

impl PortfolioState {
    pub fn new() -> Self {
        Self {
            positions: HashMap::new(),
            cash: HashMap::new(),
            realized_pnl: HashMap::new(),
            next_lot_sequence: 0,
        }
    }

    pub fn positions(&self) -> &HashMap<InstrumentId, Position> {
        &self.positions
    }

    pub fn cash(&self) -> &HashMap<Currency, Decimal> {
        &self.cash
    }

    pub fn realized_pnl(&self) -> &HashMap<Currency, Decimal> {
        &self.realized_pnl
    }

    pub fn position(&self, instrument: InstrumentId) -> Option<&Position> {
        self.positions.get(&instrument)
    }

    pub fn cash_balance(&self, currency: Currency) -> Decimal {
        self.cash.get(&currency).copied().unwrap_or(Decimal::ZERO)
    }

    pub fn realized_pnl_in(&self, currency: Currency) -> Decimal {
        self.realized_pnl
            .get(&currency)
            .copied()
            .unwrap_or(Decimal::ZERO)
    }

    pub fn currencies(&self) -> impl Iterator<Item = Currency> + '_ {
        self.cash.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state() {
        let state = PortfolioState::new();
        assert!(state.positions().is_empty());
        assert!(state.cash().is_empty());
        assert_eq!(state.cash_balance(Currency::USD), Decimal::ZERO);
        assert_eq!(state.realized_pnl_in(Currency::USD), Decimal::ZERO);
    }

    #[test]
    fn cash_lookup() {
        let mut state = PortfolioState::new();
        state.cash.insert(Currency::USD, Decimal::from(100));
        state.cash.insert(Currency::EUR, Decimal::from(50));

        assert_eq!(state.cash_balance(Currency::USD), Decimal::from(100));
        assert_eq!(state.cash_balance(Currency::EUR), Decimal::from(50));
        assert_eq!(state.cash_balance(Currency::GBP), Decimal::ZERO);
    }
}
