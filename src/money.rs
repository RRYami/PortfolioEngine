use rust_decimal::Decimal;

use crate::currency::Currency;

/// A monetary amount tagged with its currency.
///
/// Never sum or compare `Money` values with different currencies in the domain layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Money {
    pub amount: Decimal,
    pub currency: Currency,
}

impl Money {
    pub fn new(amount: Decimal, currency: Currency) -> Self {
        Self { amount, currency }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction() {
        let m = Money::new(Decimal::new(100, 2), Currency::USD);
        assert_eq!(m.amount, Decimal::new(100, 2));
        assert_eq!(m.currency, Currency::USD);
    }

    #[test]
    fn equality_same_currency() {
        let m1 = Money::new(Decimal::new(100, 2), Currency::USD);
        let m2 = Money::new(Decimal::new(100, 2), Currency::USD);
        assert_eq!(m1, m2);
    }

    #[test]
    fn equality_different_amount() {
        let m1 = Money::new(Decimal::new(100, 2), Currency::USD);
        let m2 = Money::new(Decimal::new(200, 2), Currency::USD);
        assert_ne!(m1, m2);
    }
}
