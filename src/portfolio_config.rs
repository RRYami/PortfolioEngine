use crate::currency::Currency;
use crate::lot_method::LotMethod;

/// Configuration for a single portfolio.
///
/// Drives lot-selection behavior and base-currency reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortfolioConfig {
    pub lot_method: LotMethod,
    pub base_currency: Currency,
}

impl PortfolioConfig {
    pub fn new(lot_method: LotMethod, base_currency: Currency) -> Self {
        Self {
            lot_method,
            base_currency,
        }
    }
}
