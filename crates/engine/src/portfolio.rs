use chrono::NaiveDate;

use crate::currency::Currency;
use crate::ids::{PortfolioId, UserId};
use crate::lot_method::LotMethod;

/// Metadata for a portfolio.
///
/// This is the persistent metadata record, distinct from [`PortfolioState`]
/// which is the derived state produced by folding transactions.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Portfolio {
    pub id: PortfolioId,
    /// Owner of this portfolio.
    pub user_id: UserId,
    pub name: String,
    pub base_currency: Currency,
    pub lot_method: LotMethod,
    /// Date the portfolio was opened / started tracking.
    pub inception_date: NaiveDate,
    /// Date the portfolio record was created.
    pub created_at: NaiveDate,
    /// Date the portfolio record was last updated.
    pub updated_at: NaiveDate,
}

impl Portfolio {
    pub fn new(
        id: PortfolioId,
        user_id: UserId,
        name: impl Into<String>,
        base_currency: Currency,
        lot_method: LotMethod,
        inception_date: NaiveDate,
    ) -> Self {
        let now = inception_date;
        Self {
            id,
            user_id,
            name: name.into(),
            base_currency,
            lot_method,
            inception_date,
            created_at: now,
            updated_at: now,
        }
    }
}
