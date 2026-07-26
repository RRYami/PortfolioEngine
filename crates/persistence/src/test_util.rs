//! Shared helpers for database-backed tests.
//!
//! These tests require a running Postgres with TimescaleDB (see
//! `docker-compose.yml`; `make db-up`). They run against a dedicated
//! **`ptf_engine_test`** database (auto-created on first run) so fixtures
//! never pollute the dev database / dashboard. When the database is
//! unreachable the tests skip (return `None`) instead of failing, so
//! `cargo test --workspace` stays green without Docker.

use sqlx::PgPool;

/// Test database URL: `TEST_DATABASE_URL` if set, else `DATABASE_URL` with
/// the database name swapped for `ptf_engine_test`.
fn test_database_url() -> String {
    if let Ok(url) = std::env::var("TEST_DATABASE_URL") {
        return url;
    }
    let base = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://ptf:ptf@localhost:5433/ptf_engine".to_string());
    // Replace the database name (last path segment). Any query string is
    // dropped; the dev URL has none.
    match base.rfind('/') {
        Some(i) => format!("{}/ptf_engine_test", &base[..i]),
        None => base,
    }
}

/// Returns a fresh pool for one test, or `None` when the database is
/// unavailable (tests should skip).
///
/// The pool is deliberately *not* shared between tests: each `#[tokio::test]`
/// runs on its own runtime, and pooled connections created on a dropped
/// runtime become zombies that hang the next acquirer. `max_connections` is
/// kept small so many parallel tests stay well under the server limit.
pub async fn test_pool() -> Option<PgPool> {
    match crate::create_pool_with_options(&test_database_url(), 2).await {
        Ok(pool) => Some(pool),
        Err(e) => {
            eprintln!("skipping postgres test (database unavailable): {e}");
            None
        }
    }
}
