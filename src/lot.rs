use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::ids::{LotId, TransactionId};
use crate::lot_method::LotSide;
use crate::money::Money;

/// A single tax lot.
///
/// Every lot has a positive `quantity` and a `side`. The signed economic
/// quantity is given by [`Lot::net_quantity`]:
/// - Long → positive
/// - Short → negative
///
/// `basis_per_unit` is the all-in cost per unit for longs (price + pro-rata
/// fees) and the all-in proceeds per unit for shorts (price − pro-rata fees).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lot {
    pub(crate) id: LotId,
    /// Monotonic sequence number assigned at creation for deterministic
    /// lot ordering when multiple lots share the same `open_date`.
    pub(crate) sequence: u64,
    pub(crate) side: LotSide,
    pub(crate) quantity: Decimal,
    pub(crate) basis_per_unit: Money,
    pub(crate) open_date: NaiveDate,
    pub(crate) source_transaction_id: TransactionId,
}

impl Lot {
    pub fn new(
        id: LotId,
        sequence: u64,
        side: LotSide,
        quantity: Decimal,
        basis_per_unit: Money,
        open_date: NaiveDate,
        source_transaction_id: TransactionId,
    ) -> Self {
        Self {
            id,
            sequence,
            side,
            quantity,
            basis_per_unit,
            open_date,
            source_transaction_id,
        }
    }
    pub fn id(&self) -> LotId {
        self.id
    }

    pub fn side(&self) -> LotSide {
        self.side
    }

    pub fn quantity(&self) -> Decimal {
        self.quantity
    }

    pub fn basis_per_unit(&self) -> Money {
        self.basis_per_unit
    }

    pub fn open_date(&self) -> NaiveDate {
        self.open_date
    }

    pub fn source_transaction_id(&self) -> TransactionId {
        self.source_transaction_id
    }

    /// Signed quantity: positive for long, negative for short.
    pub fn net_quantity(&self) -> Decimal {
        match self.side {
            LotSide::Long => self.quantity,
            LotSide::Short => -self.quantity,
        }
    }

    /// Total basis of this lot (`quantity * basis_per_unit`).
    ///
    /// For longs this is total cost; for shorts this is total proceeds.
    pub fn total_basis(&self) -> Money {
        Money::new(
            self.quantity * self.basis_per_unit.amount,
            self.basis_per_unit.currency,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Currency;

    fn lot(side: LotSide, qty: Decimal, basis: &str) -> Lot {
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

    #[test]
    fn long_net_quantity_positive() {
        let l = lot(LotSide::Long, Decimal::from(10), "5.00");
        assert_eq!(l.net_quantity(), Decimal::from(10));
    }

    #[test]
    fn short_net_quantity_negative() {
        let l = lot(LotSide::Short, Decimal::from(10), "5.00");
        assert_eq!(l.net_quantity(), Decimal::from(-10));
    }

    #[test]
    fn total_basis_long() {
        let l = lot(LotSide::Long, Decimal::from(10), "5.00");
        let basis = l.total_basis();
        assert_eq!(basis.amount, Decimal::from(50));
        assert_eq!(basis.currency, Currency::USD);
    }

    #[test]
    fn total_basis_short() {
        let l = lot(LotSide::Short, Decimal::from(10), "5.00");
        let basis = l.total_basis();
        assert_eq!(basis.amount, Decimal::from(50));
        assert_eq!(basis.currency, Currency::USD);
    }
}
