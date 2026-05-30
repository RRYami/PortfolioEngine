use std::collections::HashMap;

use chrono::NaiveDate;

use crate::ids::InstrumentId;
use crate::money::Money;
use crate::price::PriceError;

/// Synchronous provider for historical price time series.
///
/// Follows the same pattern as [`PriceProvider`] and [`FxRateProvider`]:
/// the domain layer stays sync; adapters batch-fetch from external
/// storage and populate a [`StaticHistoricalPriceProvider`].
pub trait HistoricalPriceProvider: Send + Sync {
    /// Return a chronological list of `(date, price)` pairs for the
    /// requested instrument covering `[from, to]` (inclusive).
    fn prices(
        &self,
        instrument: InstrumentId,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<(NaiveDate, Money)>, PriceError>;
}

/// In-memory historical price provider backed by a [`HashMap`].
#[derive(Debug, Clone, Default)]
pub struct StaticHistoricalPriceProvider {
    data: HashMap<(InstrumentId, NaiveDate), Money>,
}

impl StaticHistoricalPriceProvider {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    pub fn insert(&mut self, instrument: InstrumentId, date: NaiveDate, price: Money) {
        self.data.insert((instrument, date), price);
    }

    #[must_use]
    pub fn with_price(mut self, instrument: InstrumentId, date: NaiveDate, price: Money) -> Self {
        self.insert(instrument, date, price);
        self
    }
}

impl HistoricalPriceProvider for StaticHistoricalPriceProvider {
    fn prices(
        &self,
        instrument: InstrumentId,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<(NaiveDate, Money)>, PriceError> {
        let mut result: Vec<(NaiveDate, Money)> = self
            .data
            .iter()
            .filter(|((inst, date), _)| *inst == instrument && *date >= from && *date <= to)
            .map(|((_, date), price)| (*date, *price))
            .collect();
        result.sort_by_key(|(d, _)| *d);
        Ok(result)
    }
}
