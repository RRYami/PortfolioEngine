use rust_decimal::Decimal;

use crate::currency::Currency;
use crate::ids::{InstrumentId, LotId};
use crate::lot::Lot;
use crate::lot_method::LotSide;
use crate::money::Money;

/// A position in a single instrument, expressed as a collection of lots.
///
/// The invariant "no simultaneous long and short" is **not** enforced by this
/// type; it is enforced by the fold logic that builds positions from
/// transactions. This lets us lift the restriction later (e.g. for
/// derivatives) without a breaking change to `Position`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub(crate) instrument: InstrumentId,
    pub(crate) currency: Currency,
    pub(crate) lots: Vec<Lot>,
}

impl Position {
    pub fn new(instrument: InstrumentId, currency: Currency) -> Self {
        Self {
            instrument,
            currency,
            lots: Vec::new(),
        }
    }

    pub fn instrument(&self) -> InstrumentId {
        self.instrument
    }

    pub fn currency(&self) -> Currency {
        self.currency
    }

    pub fn lots(&self) -> &[Lot] {
        &self.lots
    }

    pub fn long_lots(&self) -> impl Iterator<Item = &Lot> {
        self.lots.iter().filter(|l| l.side() == LotSide::Long)
    }

    pub fn short_lots(&self) -> impl Iterator<Item = &Lot> {
        self.lots.iter().filter(|l| l.side() == LotSide::Short)
    }

    pub fn lot_by_id(&self, id: LotId) -> Option<&Lot> {
        self.lots.iter().find(|l| l.id() == id)
    }

    /// Sum of signed lot quantities. Positive = net long, negative = net short,
    /// zero = flat.
    pub fn net_quantity(&self) -> Decimal {
        self.lots.iter().map(|l| l.net_quantity()).sum()
    }

    pub fn is_long(&self) -> bool {
        self.net_quantity() > Decimal::ZERO
    }

    pub fn is_short(&self) -> bool {
        self.net_quantity() < Decimal::ZERO
    }

    pub fn is_flat(&self) -> bool {
        self.net_quantity() == Decimal::ZERO
    }

    pub fn is_empty(&self) -> bool {
        self.lots.is_empty()
    }

    /// Total quantity held in long lots (always >= 0).
    pub fn total_long_quantity(&self) -> Decimal {
        self.long_lots().map(|l| l.quantity()).sum()
    }

    /// Total quantity held in short lots (always >= 0).
    pub fn total_short_quantity(&self) -> Decimal {
        self.short_lots().map(|l| l.quantity()).sum()
    }

    /// Sum of `quantity * basis_per_unit` across all long lots.
    pub fn long_cost_basis(&self) -> Money {
        let amount: Decimal = self
            .long_lots()
            .map(|l| l.quantity() * l.basis_per_unit().amount)
            .sum();
        Money::new(amount, self.currency)
    }

    /// Sum of `quantity * basis_per_unit` across all short lots.
    ///
    /// For shorts, `basis_per_unit` represents proceeds at open, so this is
    /// the total proceeds received when the short position was initiated.
    pub fn short_proceeds_basis(&self) -> Money {
        let amount: Decimal = self
            .short_lots()
            .map(|l| l.quantity() * l.basis_per_unit().amount)
            .sum();
        Money::new(amount, self.currency)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::TransactionId;
    use chrono::NaiveDate;

    fn make_lot(side: LotSide, qty: Decimal, basis: &str) -> Lot {
        Lot::new(
            LotId::new(),
            0,
            side,
            qty,
            Money::new(Decimal::from_str_exact(basis).unwrap(), Currency::USD),
            NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
            TransactionId::new(),
        )
    }

    fn position_with(lots: Vec<Lot>) -> Position {
        let mut pos = Position::new(InstrumentId::new(), Currency::USD);
        pos.lots = lots;
        pos
    }

    #[test]
    fn new_is_empty() {
        let p = Position::new(InstrumentId::new(), Currency::USD);
        assert!(p.is_empty());
        assert!(p.is_flat());
        assert_eq!(
            p.long_cost_basis(),
            Money::new(Decimal::ZERO, Currency::USD)
        );
        assert_eq!(
            p.short_proceeds_basis(),
            Money::new(Decimal::ZERO, Currency::USD)
        );
    }

    #[test]
    fn flat_empty() {
        let p = position_with(vec![]);
        assert!(p.is_flat());
        assert!(!p.is_long());
        assert!(!p.is_short());
        assert_eq!(p.net_quantity(), Decimal::ZERO);
    }

    #[test]
    fn net_long() {
        let p = position_with(vec![make_lot(LotSide::Long, Decimal::from(10), "5.00")]);
        assert!(p.is_long());
        assert_eq!(p.net_quantity(), Decimal::from(10));
        assert_eq!(p.total_long_quantity(), Decimal::from(10));
        assert_eq!(p.total_short_quantity(), Decimal::ZERO);
    }

    #[test]
    fn net_short() {
        let p = position_with(vec![make_lot(LotSide::Short, Decimal::from(10), "5.00")]);
        assert!(p.is_short());
        assert_eq!(p.net_quantity(), Decimal::from(-10));
        assert_eq!(p.total_long_quantity(), Decimal::ZERO);
        assert_eq!(p.total_short_quantity(), Decimal::from(10));
    }

    #[test]
    fn mixed_long_and_short() {
        // Allowed at the type level; fold logic may forbid it in v1.
        let p = position_with(vec![
            make_lot(LotSide::Long, Decimal::from(10), "5.00"),
            make_lot(LotSide::Short, Decimal::from(3), "6.00"),
        ]);
        assert!(p.is_long());
        assert_eq!(p.net_quantity(), Decimal::from(7));
        assert_eq!(p.total_long_quantity(), Decimal::from(10));
        assert_eq!(p.total_short_quantity(), Decimal::from(3));
    }

    #[test]
    fn multiple_long_lots_sum() {
        let p = position_with(vec![
            make_lot(LotSide::Long, Decimal::from(5), "5.00"),
            make_lot(LotSide::Long, Decimal::from(7), "10.00"),
        ]);
        assert_eq!(p.net_quantity(), Decimal::from(12));
        assert_eq!(p.total_long_quantity(), Decimal::from(12));
    }

    #[test]
    fn long_cost_basis_single_lot() {
        let p = position_with(vec![make_lot(LotSide::Long, Decimal::from(10), "5.00")]);
        assert_eq!(
            p.long_cost_basis(),
            Money::new(Decimal::from(50), Currency::USD)
        );
    }

    #[test]
    fn long_cost_basis_multiple_lots() {
        let p = position_with(vec![
            make_lot(LotSide::Long, Decimal::from(10), "5.00"),
            make_lot(LotSide::Long, Decimal::from(5), "8.00"),
        ]);
        // 10*5 + 5*8 = 50 + 40 = 90
        assert_eq!(
            p.long_cost_basis(),
            Money::new(Decimal::from(90), Currency::USD)
        );
    }

    #[test]
    fn long_cost_basis_with_short_lots_ignored() {
        let p = position_with(vec![
            make_lot(LotSide::Long, Decimal::from(10), "5.00"),
            make_lot(LotSide::Short, Decimal::from(3), "6.00"),
        ]);
        assert_eq!(
            p.long_cost_basis(),
            Money::new(Decimal::from(50), Currency::USD)
        );
    }

    #[test]
    fn short_proceeds_basis_single_lot() {
        let p = position_with(vec![make_lot(LotSide::Short, Decimal::from(10), "5.00")]);
        assert_eq!(
            p.short_proceeds_basis(),
            Money::new(Decimal::from(50), Currency::USD)
        );
    }

    #[test]
    fn short_proceeds_basis_multiple_lots() {
        let p = position_with(vec![
            make_lot(LotSide::Short, Decimal::from(10), "5.00"),
            make_lot(LotSide::Short, Decimal::from(5), "8.00"),
        ]);
        // 10*5 + 5*8 = 50 + 40 = 90
        assert_eq!(
            p.short_proceeds_basis(),
            Money::new(Decimal::from(90), Currency::USD)
        );
    }

    #[test]
    fn short_proceeds_basis_with_long_lots_ignored() {
        let p = position_with(vec![
            make_lot(LotSide::Long, Decimal::from(10), "5.00"),
            make_lot(LotSide::Short, Decimal::from(3), "6.00"),
        ]);
        assert_eq!(
            p.short_proceeds_basis(),
            Money::new(Decimal::from(18), Currency::USD)
        );
    }

    #[test]
    fn lot_by_id_found() {
        let lot = make_lot(LotSide::Long, Decimal::from(10), "5.00");
        let id = lot.id();
        let p = position_with(vec![lot]);
        assert!(p.lot_by_id(id).is_some());
    }

    #[test]
    fn lot_by_id_not_found() {
        let p = position_with(vec![make_lot(LotSide::Long, Decimal::from(10), "5.00")]);
        assert!(p.lot_by_id(LotId::new()).is_none());
    }
}
