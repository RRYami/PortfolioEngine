//! Postgres-backed [`TransactionRepository`].
//!
//! Rows live in the `transactions` `TimescaleDB` hypertable, partitioned by
//! `trade_date`. Chronological order is `ORDER BY trade_date, seq` where
//! `seq` is the identity column recording insertion order — the same
//! tiebreak the in-memory implementation uses.

use async_trait::async_trait;
use chrono::NaiveDate;
use sqlx::PgPool;
use sqlx::types::Json;
use uuid::Uuid;

use ptf_engine::ids::{PortfolioId, TransactionId};
use ptf_engine::repository::error::RepoError;
use ptf_engine::repository::transaction::TransactionRepository;
use ptf_engine::transaction::{Transaction, TransactionKind};

use crate::error::map_sqlx;

type TransactionRow = (Uuid, NaiveDate, NaiveDate, Json<TransactionKind>);

fn row_to_transaction(row: TransactionRow) -> Transaction {
    let (id, trade_date, settle_date, Json(kind)) = row;
    Transaction {
        id: TransactionId(id),
        trade_date,
        settle_date,
        kind,
    }
}

/// Postgres-backed transaction repository.
#[derive(Debug, Clone)]
pub struct PgTransactionRepository {
    pool: PgPool,
}

impl PgTransactionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl TransactionRepository for PgTransactionRepository {
    /// Appends a transaction. The portfolio must exist: a foreign-key
    /// violation surfaces as [`RepoError::NotFound`].
    async fn append(
        &self,
        portfolio_id: PortfolioId,
        transaction: &Transaction,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO transactions (id, portfolio_id, trade_date, settle_date, kind)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(transaction.id.0)
        .bind(portfolio_id.0)
        .bind(transaction.trade_date)
        .bind(transaction.settle_date)
        .bind(Json(&transaction.kind))
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    async fn list(&self, portfolio_id: PortfolioId) -> Result<Vec<Transaction>, RepoError> {
        let rows = sqlx::query_as::<_, TransactionRow>(
            "SELECT id, trade_date, settle_date, kind FROM transactions
             WHERE portfolio_id = $1
             ORDER BY trade_date, seq",
        )
        .bind(portfolio_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(row_to_transaction).collect())
    }

    async fn list_until(
        &self,
        portfolio_id: PortfolioId,
        as_of: NaiveDate,
    ) -> Result<Vec<Transaction>, RepoError> {
        let rows = sqlx::query_as::<_, TransactionRow>(
            "SELECT id, trade_date, settle_date, kind FROM transactions
             WHERE portfolio_id = $1 AND trade_date <= $2
             ORDER BY trade_date, seq",
        )
        .bind(portfolio_id.0)
        .bind(as_of)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(row_to_transaction).collect())
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use ptf_engine::currency::Currency;
    use ptf_engine::ids::TransactionId;
    use ptf_engine::lot_method::LotMethod;
    use ptf_engine::money::Money;
    use ptf_engine::portfolio::Portfolio;
    use ptf_engine::repository::portfolio::PortfolioRepository;
    use ptf_engine::transaction::TransactionKind;

    use super::*;
    use crate::portfolio::PgPortfolioRepository;
    use crate::test_util::test_pool;

    fn usd(dollars: &str) -> Money {
        Money::new(Decimal::from_str_exact(dollars).unwrap(), Currency::USD)
    }

    fn tx(date: NaiveDate, kind: TransactionKind) -> Transaction {
        Transaction::new(TransactionId::new(), date, date, kind).unwrap()
    }

    /// Creates a fresh portfolio (transactions reference it via FK).
    async fn new_portfolio(pool: &PgPool) -> PortfolioId {
        let repo = PgPortfolioRepository::new(pool.clone());
        let p = Portfolio::new(
            PortfolioId::new(),
            format!("tx-test-{}", Uuid::new_v4()),
            Currency::USD,
            LotMethod::Fifo,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        );
        repo.create(&p).await.unwrap();
        p.id
    }

    #[tokio::test]
    async fn transaction_append_and_list() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let pid = new_portfolio(&pool).await;
        let repo = PgTransactionRepository::new(pool);
        let d = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let tx = tx(d, TransactionKind::deposit(usd("100.00")).unwrap());

        repo.append(pid, &tx).await.unwrap();
        let list = repo.list(pid).await.unwrap();
        assert_eq!(list, vec![tx]);
    }

    #[tokio::test]
    async fn transaction_append_missing_portfolio_returns_not_found() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgTransactionRepository::new(pool);
        let d = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let tx = tx(d, TransactionKind::deposit(usd("100.00")).unwrap());

        let result = repo.append(PortfolioId::new(), &tx).await;
        assert!(matches!(result, Err(RepoError::NotFound)));
    }

    #[tokio::test]
    async fn transaction_list_in_chronological_order() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let pid = new_portfolio(&pool).await;
        let repo = PgTransactionRepository::new(pool);
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d3 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();

        let tx1 = tx(d1, TransactionKind::deposit(usd("100.00")).unwrap());
        let tx2 = tx(d2, TransactionKind::deposit(usd("200.00")).unwrap());
        let tx3 = tx(d3, TransactionKind::deposit(usd("300.00")).unwrap());

        // Append out of order.
        repo.append(pid, &tx1).await.unwrap();
        repo.append(pid, &tx2).await.unwrap();
        repo.append(pid, &tx3).await.unwrap();

        let list = repo.list(pid).await.unwrap();
        assert_eq!(list, vec![tx2, tx3, tx1]);
    }

    #[tokio::test]
    async fn transaction_list_same_day_preserves_insertion_order() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let pid = new_portfolio(&pool).await;
        let repo = PgTransactionRepository::new(pool);
        let d = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();

        let tx1 = tx(d, TransactionKind::deposit(usd("100.00")).unwrap());
        let tx2 = tx(d, TransactionKind::deposit(usd("200.00")).unwrap());
        let tx3 = tx(d, TransactionKind::deposit(usd("300.00")).unwrap());

        repo.append(pid, &tx1).await.unwrap();
        repo.append(pid, &tx2).await.unwrap();
        repo.append(pid, &tx3).await.unwrap();

        let list = repo.list(pid).await.unwrap();
        assert_eq!(list, vec![tx1, tx2, tx3]);
    }

    #[tokio::test]
    async fn transaction_list_until_excludes_after_date() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let pid = new_portfolio(&pool).await;
        let repo = PgTransactionRepository::new(pool);
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d3 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();

        let tx1 = tx(d1, TransactionKind::deposit(usd("100.00")).unwrap());
        let tx2 = tx(d2, TransactionKind::deposit(usd("200.00")).unwrap());
        let tx3 = tx(d3, TransactionKind::deposit(usd("300.00")).unwrap());

        repo.append(pid, &tx1).await.unwrap();
        repo.append(pid, &tx2).await.unwrap();
        repo.append(pid, &tx3).await.unwrap();

        let list = repo.list_until(pid, d2).await.unwrap();
        assert_eq!(list, vec![tx1, tx2]);
    }

    #[tokio::test]
    async fn transaction_list_isolated_per_portfolio() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgTransactionRepository::new(pool.clone());
        let pid_a = new_portfolio(&pool).await;
        let pid_b = new_portfolio(&pool).await;
        let d = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let tx = tx(d, TransactionKind::deposit(usd("100.00")).unwrap());

        repo.append(pid_a, &tx).await.unwrap();
        let list_b = repo.list(pid_b).await.unwrap();
        assert!(list_b.is_empty());
    }

    #[tokio::test]
    async fn transaction_delete_portfolio_cascades() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let portfolios = PgPortfolioRepository::new(pool.clone());
        let repo = PgTransactionRepository::new(pool.clone());
        let pid = new_portfolio(&pool).await;
        let d = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let tx = tx(d, TransactionKind::deposit(usd("100.00")).unwrap());

        repo.append(pid, &tx).await.unwrap();
        portfolios.delete(pid).await.unwrap();

        let list = repo.list(pid).await.unwrap();
        assert!(list.is_empty());
    }
}
