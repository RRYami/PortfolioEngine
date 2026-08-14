//! Postgres persistence implementations for ptf-engine repository traits.

use std::time::Duration;

use sqlx::PgPool;
use sqlx::migrate::MigrateDatabase;

pub mod error;
pub mod instrument;
pub mod portfolio;
#[cfg(test)]
mod test_util;
pub mod transaction;
pub mod user;

pub use instrument::PgInstrumentRepository;
pub use portfolio::PgPortfolioRepository;
pub use transaction::PgTransactionRepository;
pub use user::PgUserRepository;

pub const MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Error type for persistence operations.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("serialization error: {0}")]
    Serialization(String),
}

/// Creates the database if it does not exist. Tolerates concurrent creators:
/// a `duplicate_database` error (`42P04`) from a lost race is success.
async fn ensure_database(database_url: &str) -> Result<(), PersistenceError> {
    if sqlx::Postgres::database_exists(database_url)
        .await
        .unwrap_or(false)
    {
        return Ok(());
    }
    match sqlx::Postgres::create_database(database_url).await {
        Ok(()) => Ok(()),
        Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("42P04") => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Creates a connection pool and runs embedded migrations.
///
/// # Errors
/// Returns `PersistenceError::Migration` if migrations fail,
/// or `PersistenceError::Database` if the pool cannot be created.
pub async fn create_pool(database_url: &str) -> Result<PgPool, PersistenceError> {
    ensure_database(database_url).await?;

    let pool = PgPool::connect(database_url).await?;

    MIGRATIONS.run(&pool).await?;

    Ok(pool)
}

/// Creates a connection pool with connection limits and timeout settings.
///
/// # Errors
/// Returns `PersistenceError::Migration` if migrations fail,
/// or `PersistenceError::Database` if the pool cannot be created.
pub async fn create_pool_with_options(
    database_url: &str,
    max_connections: u32,
) -> Result<PgPool, PersistenceError> {
    ensure_database(database_url).await?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect(database_url)
        .await?;

    MIGRATIONS.run(&pool).await?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests require a running Postgres (see docker-compose.yml) and skip
    // gracefully when it is unavailable.
    // Run with: make db-up && cargo test -p ptf-persistence

    #[tokio::test]
    async fn pool_creation_and_migration_succeeds() {
        let Some(pool) = test_util::test_pool().await else {
            return;
        };

        // Verify tables exist
        let tables: Vec<(String,)> = sqlx::query_as(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'",
        )
        .fetch_all(&pool)
        .await
        .expect("query failed");

        let table_names: Vec<String> = tables.into_iter().map(|t| t.0).collect();
        assert!(table_names.contains(&"portfolios".to_string()));
        assert!(table_names.contains(&"instruments".to_string()));
        assert!(table_names.contains(&"transactions".to_string()));
        assert!(table_names.contains(&"users".to_string()));
    }

    #[tokio::test]
    async fn transactions_is_a_hypertable() {
        let Some(pool) = test_util::test_pool().await else {
            return;
        };

        let hypertables: Vec<(String,)> =
            sqlx::query_as("SELECT hypertable_name FROM timescaledb_information.hypertables")
                .fetch_all(&pool)
                .await
                .expect("query failed");

        let names: Vec<String> = hypertables.into_iter().map(|t| t.0).collect();
        assert!(names.contains(&"transactions".to_string()));

        // Reference data must stay plain tables.
        assert!(!names.contains(&"portfolios".to_string()));
        assert!(!names.contains(&"instruments".to_string()));
    }
}
