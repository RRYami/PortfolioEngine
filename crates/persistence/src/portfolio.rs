//! Postgres-backed [`PortfolioRepository`].

use async_trait::async_trait;
use chrono::NaiveDate;
use sqlx::PgPool;
use sqlx::types::Json;
use uuid::Uuid;

use ptf_engine::currency::Currency;
use ptf_engine::ids::PortfolioId;
use ptf_engine::lot_method::LotMethod;
use ptf_engine::portfolio::Portfolio;
use ptf_engine::repository::error::RepoError;
use ptf_engine::repository::portfolio::PortfolioRepository;

use crate::error::{map_domain, map_sqlx};

type PortfolioRow = (
    Uuid,
    String,
    String,
    Json<LotMethod>,
    NaiveDate,
    NaiveDate,
    NaiveDate,
);

fn row_to_portfolio(row: PortfolioRow) -> Result<Portfolio, RepoError> {
    let (id, name, base_currency, Json(lot_method), inception_date, created_at, updated_at) = row;
    Ok(Portfolio {
        id: PortfolioId(id),
        name,
        base_currency: Currency::try_from(base_currency.as_str()).map_err(|e| map_domain(&e))?,
        lot_method,
        inception_date,
        created_at,
        updated_at,
    })
}

/// Postgres-backed portfolio metadata repository.
#[derive(Debug, Clone)]
pub struct PgPortfolioRepository {
    pool: PgPool,
}

impl PgPortfolioRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PortfolioRepository for PgPortfolioRepository {
    async fn create(&self, portfolio: &Portfolio) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO portfolios
                 (id, name, base_currency, lot_method, inception_date, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(portfolio.id.0)
        .bind(&portfolio.name)
        .bind(portfolio.base_currency.as_str())
        .bind(Json(portfolio.lot_method))
        .bind(portfolio.inception_date)
        .bind(portfolio.created_at)
        .bind(portfolio.updated_at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    async fn get(&self, id: PortfolioId) -> Result<Portfolio, RepoError> {
        let row = sqlx::query_as::<_, PortfolioRow>(
            "SELECT id, name, base_currency, lot_method, inception_date, created_at, updated_at
             FROM portfolios WHERE id = $1",
        )
        .bind(id.0)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx)?;
        row_to_portfolio(row)
    }

    async fn list(&self) -> Result<Vec<Portfolio>, RepoError> {
        let rows = sqlx::query_as::<_, PortfolioRow>(
            "SELECT id, name, base_currency, lot_method, inception_date, created_at, updated_at
             FROM portfolios ORDER BY created_at, id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx)?;
        rows.into_iter().map(row_to_portfolio).collect()
    }

    async fn update(&self, portfolio: &Portfolio) -> Result<(), RepoError> {
        let result = sqlx::query(
            "UPDATE portfolios SET
                 name = $2, base_currency = $3, lot_method = $4,
                 inception_date = $5, created_at = $6, updated_at = $7
             WHERE id = $1",
        )
        .bind(portfolio.id.0)
        .bind(&portfolio.name)
        .bind(portfolio.base_currency.as_str())
        .bind(Json(portfolio.lot_method))
        .bind(portfolio.inception_date)
        .bind(portfolio.created_at)
        .bind(portfolio.updated_at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        if result.rows_affected() == 0 {
            return Err(RepoError::NotFound);
        }
        Ok(())
    }

    async fn delete(&self, id: PortfolioId) -> Result<(), RepoError> {
        let result = sqlx::query("DELETE FROM portfolios WHERE id = $1")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx)?;
        if result.rows_affected() == 0 {
            return Err(RepoError::NotFound);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::test_pool;

    fn portfolio(name: &str) -> Portfolio {
        Portfolio::new(
            PortfolioId::new(),
            name,
            Currency::USD,
            LotMethod::Fifo,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        )
    }

    #[tokio::test]
    async fn portfolio_create_and_get() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgPortfolioRepository::new(pool);
        let p = portfolio("test-portfolio");

        repo.create(&p).await.unwrap();
        let got = repo.get(p.id).await.unwrap();
        assert_eq!(got, p);
    }

    #[tokio::test]
    async fn portfolio_create_duplicate_errors() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgPortfolioRepository::new(pool);
        let p = portfolio("dupe");

        repo.create(&p).await.unwrap();
        let result = repo.create(&p).await;
        assert!(matches!(result, Err(RepoError::AlreadyExists(_))));
    }

    #[tokio::test]
    async fn portfolio_get_missing_returns_not_found() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgPortfolioRepository::new(pool);
        let result = repo.get(PortfolioId::new()).await;
        assert!(matches!(result, Err(RepoError::NotFound)));
    }

    #[tokio::test]
    async fn portfolio_list_contains_created() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgPortfolioRepository::new(pool);
        // Unique names: other tests may insert into the shared table in parallel.
        let p1 = portfolio(&format!("alpha-{}", Uuid::new_v4()));
        let p2 = portfolio(&format!("beta-{}", Uuid::new_v4()));

        repo.create(&p1).await.unwrap();
        repo.create(&p2).await.unwrap();

        let list = repo.list().await.unwrap();
        assert!(list.contains(&p1));
        assert!(list.contains(&p2));
    }

    #[tokio::test]
    async fn portfolio_update() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgPortfolioRepository::new(pool);
        let mut p = portfolio("original");
        repo.create(&p).await.unwrap();

        p.name = "updated".to_string();
        p.lot_method = LotMethod::Lifo;
        repo.update(&p).await.unwrap();

        let got = repo.get(p.id).await.unwrap();
        assert_eq!(got, p);
    }

    #[tokio::test]
    async fn portfolio_update_missing_returns_not_found() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgPortfolioRepository::new(pool);
        let p = portfolio("orphan");
        let result = repo.update(&p).await;
        assert!(matches!(result, Err(RepoError::NotFound)));
    }

    #[tokio::test]
    async fn portfolio_delete() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgPortfolioRepository::new(pool);
        let p = portfolio("to-delete");
        repo.create(&p).await.unwrap();

        repo.delete(p.id).await.unwrap();
        assert!(matches!(repo.get(p.id).await, Err(RepoError::NotFound)));
    }

    #[tokio::test]
    async fn portfolio_delete_missing_returns_not_found() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgPortfolioRepository::new(pool);
        let result = repo.delete(PortfolioId::new()).await;
        assert!(matches!(result, Err(RepoError::NotFound)));
    }
}
