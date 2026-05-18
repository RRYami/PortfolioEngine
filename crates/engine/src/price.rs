use std::collections::HashMap;

use chrono::NaiveDate;

use crate::ids::InstrumentId;
use crate::money::Money;

/// Errors returned by price providers.
#[derive(Debug, thiserror::Error)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PriceError {
    #[error("price unavailable for {instrument:?} on {date}")]
    PriceUnavailable {
        instrument: InstrumentId,
        date: NaiveDate,
    },
    #[error("provider error: {0}")]
    ProviderError(String),
}

/// Synchronous price provider.
///
/// Returns a [`Money`] so the currency of the quoted price is carried along
/// for free; callers do not need to track instrument currencies separately.
pub trait PriceProvider: Send + Sync {
    fn price(&self, instrument: InstrumentId, date: NaiveDate) -> Result<Money, PriceError>;
}

/// In-memory price provider backed by a [`HashMap`].
#[derive(Debug, Clone, Default)]
pub struct StaticPriceProvider {
    prices: HashMap<(InstrumentId, NaiveDate), Money>,
}

impl StaticPriceProvider {
    pub fn new() -> Self {
        Self {
            prices: HashMap::new(),
        }
    }

    pub fn insert(&mut self, instrument: InstrumentId, date: NaiveDate, price: Money) {
        self.prices.insert((instrument, date), price);
    }

    #[must_use]
    pub fn with_price(mut self, instrument: InstrumentId, date: NaiveDate, price: Money) -> Self {
        self.insert(instrument, date, price);
        self
    }
}

impl PriceProvider for StaticPriceProvider {
    fn price(&self, instrument: InstrumentId, date: NaiveDate) -> Result<Money, PriceError> {
        self.prices
            .get(&(instrument, date))
            .copied()
            .ok_or(PriceError::PriceUnavailable { instrument, date })
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;
    use crate::currency::Currency;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 6, 15).unwrap()
    }

    #[test]
    fn static_hit() {
        let id = InstrumentId::new();
        let provider = StaticPriceProvider::new().with_price(
            id,
            date(),
            Money::new(Decimal::from(150), Currency::USD),
        );

        let price = provider.price(id, date()).unwrap();
        assert_eq!(price.amount, Decimal::from(150));
        assert_eq!(price.currency, Currency::USD);
    }

    #[test]
    fn static_miss() {
        let id = InstrumentId::new();
        let provider = StaticPriceProvider::new();

        assert!(matches!(
            provider.price(id, date()),
            Err(PriceError::PriceUnavailable { .. })
        ));
    }

    #[test]
    fn static_different_instrument_misses() {
        let id_a = InstrumentId::new();
        let id_b = InstrumentId::new();
        let provider = StaticPriceProvider::new().with_price(
            id_a,
            date(),
            Money::new(Decimal::from(150), Currency::USD),
        );

        assert!(matches!(
            provider.price(id_b, date()),
            Err(PriceError::PriceUnavailable { .. })
        ));
    }

    #[test]
    fn static_different_date_misses() {
        let id = InstrumentId::new();
        let provider = StaticPriceProvider::new().with_price(
            id,
            date(),
            Money::new(Decimal::from(150), Currency::USD),
        );

        let other = NaiveDate::from_ymd_opt(2024, 6, 16).unwrap();
        assert!(matches!(
            provider.price(id, other),
            Err(PriceError::PriceUnavailable { .. })
        ));
    }
}
