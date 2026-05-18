use async_trait::async_trait;
use chrono::NaiveDate;

use crate::ids::PortfolioId;
use crate::transaction::Transaction;

use super::error::RepoError;

/// Repository for portfolio transactions.
///
/// [`list`](TransactionRepository::list) returns transactions in chronological
/// order (by `trade_date`, tie-broken by insertion order).
#[async_trait]
pub trait TransactionRepository: Send + Sync {
    async fn append(
        &self,
        portfolio_id: PortfolioId,
        transaction: &Transaction,
    ) -> Result<(), RepoError>;

    /// Returns all transactions for a portfolio in chronological order
    /// (by `trade_date`, tie-broken by insertion order).
    async fn list(&self, portfolio_id: PortfolioId) -> Result<Vec<Transaction>, RepoError>;

    /// Returns transactions up to and including `as_of` date, in chronological
    /// order (by `trade_date`, tie-broken by insertion order).
    async fn list_until(
        &self,
        portfolio_id: PortfolioId,
        as_of: NaiveDate,
    ) -> Result<Vec<Transaction>, RepoError>;
}
