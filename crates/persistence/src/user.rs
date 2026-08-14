//! Postgres-backed [`UserRepository`].

use async_trait::async_trait;
use chrono::NaiveDate;
use sqlx::PgPool;
use uuid::Uuid;

use ptf_engine::ids::UserId;
use ptf_engine::repository::error::RepoError;
use ptf_engine::repository::user::UserRepository;
use ptf_engine::user::User;

use crate::error::map_sqlx;

type UserRow = (Uuid, String, String, NaiveDate, NaiveDate);

fn row_to_user(row: UserRow) -> User {
    let (id, email, password_hash, created_at, updated_at) = row;
    User {
        id: UserId(id),
        email,
        password_hash,
        created_at,
        updated_at,
    }
}

/// Postgres-backed user account repository.
#[derive(Debug, Clone)]
pub struct PgUserRepository {
    pool: PgPool,
}

impl PgUserRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl UserRepository for PgUserRepository {
    /// Creates a user. Emails are stored lowercase (see [`User::new`]); the
    /// unique index turns a case-insensitive duplicate into
    /// [`RepoError::AlreadyExists`].
    async fn create(&self, user: &User) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(user.id.0)
        .bind(user.email.to_lowercase())
        .bind(&user.password_hash)
        .bind(user.created_at)
        .bind(user.updated_at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    async fn get(&self, id: UserId) -> Result<User, RepoError> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, created_at, updated_at
             FROM users WHERE id = $1",
        )
        .bind(id.0)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(row_to_user(row))
    }

    async fn by_email(&self, email: &str) -> Result<User, RepoError> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, created_at, updated_at
             FROM users WHERE email = $1",
        )
        .bind(email.to_lowercase())
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(row_to_user(row))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::test_pool;

    fn user(email: &str) -> User {
        User::new(
            UserId::new(),
            email,
            "hash",
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        )
    }

    /// Unique email per call: the users table is shared across parallel tests
    /// and persists between runs.
    fn unique_email() -> String {
        format!("{}@example.com", Uuid::new_v4().simple())
    }

    #[tokio::test]
    async fn user_create_and_get() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgUserRepository::new(pool);
        let u = user(&unique_email());

        repo.create(&u).await.unwrap();
        let got = repo.get(u.id).await.unwrap();
        assert_eq!(got, u);
    }

    #[tokio::test]
    async fn user_by_email_is_case_insensitive() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgUserRepository::new(pool);
        let email = unique_email();
        let u = user(&email);

        repo.create(&u).await.unwrap();
        let got = repo.by_email(&email.to_uppercase()).await.unwrap();
        assert_eq!(got, u);
    }

    #[tokio::test]
    async fn user_duplicate_email_rejected_case_insensitively() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgUserRepository::new(pool);
        let email = unique_email();
        let u1 = user(&email);
        let u2 = user(&email.to_uppercase());

        repo.create(&u1).await.unwrap();
        let result = repo.create(&u2).await;
        assert!(matches!(result, Err(RepoError::AlreadyExists(_))));
    }

    #[tokio::test]
    async fn user_get_missing_returns_not_found() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let repo = PgUserRepository::new(pool);
        assert!(matches!(
            repo.get(UserId::new()).await,
            Err(RepoError::NotFound)
        ));
        assert!(matches!(
            repo.by_email("nobody@example.com").await,
            Err(RepoError::NotFound)
        ));
    }
}
