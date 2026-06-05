//! Postgres persistence implementations for ptf-engine repository traits.

use std::time::Duration;

use sqlx::PgPool;
use sqlx::migrate::MigrateDatabase;

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

/// Creates a connection pool and runs embedded migrations.
///
/// # Errors
/// Returns `PersistenceError::Migration` if migrations fail,
/// or `PersistenceError::Database` if the pool cannot be created.
pub async fn create_pool(database_url: &str) -> Result<PgPool, PersistenceError> {
    if !sqlx::Postgres::database_exists(database_url)
        .await
        .unwrap_or(false)
    {
        sqlx::Postgres::create_database(database_url).await?;
    }

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
    if !sqlx::Postgres::database_exists(database_url)
        .await
        .unwrap_or(false)
    {
        sqlx::Postgres::create_database(database_url).await?;
    }

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

    // These tests require a running Postgres (see docker-compose.yml).
    // Run with: DATABASE_URL=postgres://ptf:ptf@localhost:5433/ptf_engine cargo test -p ptf-persistence

    #[tokio::test]
    async fn pool_creation_and_migration_succeeds() {
        let database_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://ptf:ptf@localhost:5433/ptf_engine".to_string());

        let pool = create_pool(&database_url)
            .await
            .expect("pool creation failed");

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
    }
}
