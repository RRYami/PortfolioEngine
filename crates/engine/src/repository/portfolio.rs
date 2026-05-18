use async_trait::async_trait;

use crate::ids::PortfolioId;
use crate::portfolio::Portfolio;

use super::error::RepoError;

/// Repository for portfolio metadata.
#[async_trait]
pub trait PortfolioRepository: Send + Sync {
    async fn create(&self, portfolio: &Portfolio) -> Result<(), RepoError>;
    async fn get(&self, id: PortfolioId) -> Result<Portfolio, RepoError>;
    async fn list(&self) -> Result<Vec<Portfolio>, RepoError>;
    async fn update(&self, portfolio: &Portfolio) -> Result<(), RepoError>;
    async fn delete(&self, id: PortfolioId) -> Result<(), RepoError>;
}
