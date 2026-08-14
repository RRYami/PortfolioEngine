use async_trait::async_trait;

use crate::ids::{PortfolioId, UserId};
use crate::portfolio::Portfolio;

use super::error::RepoError;

/// Repository for portfolio metadata.
///
/// Portfolios are owned by users: [`list`](PortfolioRepository::list) returns
/// only the given owner's portfolios. Cross-user access control is enforced
/// by callers (compare [`Portfolio::user_id`]).
#[async_trait]
pub trait PortfolioRepository: Send + Sync {
    async fn create(&self, portfolio: &Portfolio) -> Result<(), RepoError>;
    async fn get(&self, id: PortfolioId) -> Result<Portfolio, RepoError>;
    /// Returns all portfolios owned by `user_id`.
    async fn list(&self, user_id: UserId) -> Result<Vec<Portfolio>, RepoError>;
    async fn update(&self, portfolio: &Portfolio) -> Result<(), RepoError>;
    async fn delete(&self, id: PortfolioId) -> Result<(), RepoError>;
}
