//! Postgres-backed [`InstrumentRepository`].

use async_trait::async_trait;
use sqlx::PgPool;
use sqlx::types::Json;
use uuid::Uuid;

use ptf_engine::currency::Currency;
use ptf_engine::ids::InstrumentId;
use ptf_engine::instrument::{Instrument, InstrumentKind};
use ptf_engine::repository::error::RepoError;
use ptf_engine::repository::instrument::InstrumentRepository;

use crate::error::{map_domain, map_sqlx};

type InstrumentRow = (Uuid, String, String, String, Json<InstrumentKind>);

fn row_to_instrument(row: InstrumentRow) -> Result<Instrument, RepoError> {
    let (id, symbol, name, currency, Json(kind)) = row;
    Ok(Instrument {
        id: InstrumentId(id),
        symbol,
        name,
        currency: Currency::try_from(currency.as_str()).map_err(|e| map_domain(&e))?,
        kind,
    })
}

/// Postgres-backed instrument reference-data repository.
#[derive(Debug, Clone)]
pub struct PgInstrumentRepository {
    pool: PgPool,
}

impl PgInstrumentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl InstrumentRepository for PgInstrumentRepository {
    /// Upserts by id. A symbol belonging to a *different* id violates the
    /// unique index and surfaces as [`RepoError::AlreadyExists`].
    async fn upsert(&self, instrument: &Instrument) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO instruments (id, symbol, name, currency, kind)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (id) DO UPDATE SET
                 symbol = EXCLUDED.symbol,
                 name = EXCLUDED.name,
                 currency = EXCLUDED.currency,
                 kind = EXCLUDED.kind",
        )
        .bind(instrument.id.0)
        .bind(&instrument.symbol)
        .bind(&instrument.name)
        .bind(instrument.currency.as_str())
        .bind(Json(instrument.kind))
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    async fn get(&self, id: InstrumentId) -> Result<Instrument, RepoError> {
        let row = sqlx::query_as::<_, InstrumentRow>(
            "SELECT id, symbol, name, currency, kind FROM instruments WHERE id = $1",
        )
        .bind(id.0)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx)?;
        row_to_instrument(row)
    }

    async fn by_symbol(&self, symbol: &str) -> Result<Instrument, RepoError> {
        let row = sqlx::query_as::<_, InstrumentRow>(
            "SELECT id, symbol, name, currency, kind FROM instruments WHERE symbol = $1",
        )
        .bind(symbol)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx)?;
        row_to_instrument(row)
    }

    async fn list(&self) -> Result<Vec<Instrument>, RepoError> {
        let rows = sqlx::query_as::<_, InstrumentRow>(
            "SELECT id, symbol, name, currency, kind FROM instruments ORDER BY symbol",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx)?;
        rows.into_iter().map(row_to_instrument).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::test_pool;

    fn instrument(symbol: &str) -> Instrument {
        Instrument {
            id: InstrumentId::new(),
            symbol: symbol.to_string(),
            name: symbol.to_string(),
            currency: Currency::USD,
            kind: InstrumentKind::Equity {},
        }
    }

    /// Unique symbol per call: the instruments table is shared across
    /// parallel tests and persists between runs.
    fn unique_symbol() -> String {
        let mut s: String = Uuid::new_v4()
            .simple()
            .to_string()
            .chars()
            .take(10)
            .collect();
        s.make_ascii_uppercase();
        s
    }

    #[tokio::test]
    async fn instrument_upsert_and_get() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgInstrumentRepository::new(pool);
        let inst = instrument(&unique_symbol());

        repo.upsert(&inst).await.unwrap();
        let got = repo.get(inst.id).await.unwrap();
        assert_eq!(got, inst);
    }

    #[tokio::test]
    async fn instrument_by_symbol() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgInstrumentRepository::new(pool);
        let inst = instrument(&unique_symbol());

        repo.upsert(&inst).await.unwrap();
        let got = repo.by_symbol(&inst.symbol).await.unwrap();
        assert_eq!(got, inst);
    }

    #[tokio::test]
    async fn instrument_by_symbol_missing_returns_not_found() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgInstrumentRepository::new(pool);
        let result = repo.by_symbol("NO_SUCH_SYMBOL").await;
        assert!(matches!(result, Err(RepoError::NotFound)));
    }

    #[tokio::test]
    async fn instrument_list_contains_created() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgInstrumentRepository::new(pool);
        let i1 = instrument(&unique_symbol());
        let i2 = instrument(&unique_symbol());

        repo.upsert(&i1).await.unwrap();
        repo.upsert(&i2).await.unwrap();

        let list = repo.list().await.unwrap();
        assert!(list.contains(&i1));
        assert!(list.contains(&i2));
    }

    #[tokio::test]
    async fn instrument_symbol_uniqueness_rejected() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgInstrumentRepository::new(pool);
        let symbol = unique_symbol();
        let i1 = instrument(&symbol);
        let i2 = Instrument {
            id: InstrumentId::new(), // same symbol, different id
            ..instrument(&symbol)
        };

        repo.upsert(&i1).await.unwrap();
        let result = repo.upsert(&i2).await;
        assert!(matches!(result, Err(RepoError::AlreadyExists(_))));
    }

    #[tokio::test]
    async fn instrument_upsert_same_id_updates() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgInstrumentRepository::new(pool);
        let mut i1 = instrument(&unique_symbol());
        repo.upsert(&i1).await.unwrap();

        i1.name = "Apple Inc.".to_string();
        repo.upsert(&i1).await.unwrap();

        let got = repo.get(i1.id).await.unwrap();
        assert_eq!(got, i1);
    }
}
