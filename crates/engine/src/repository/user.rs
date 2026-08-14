use async_trait::async_trait;

use crate::ids::UserId;
use crate::user::User;

use super::error::RepoError;

/// Repository for user accounts.
///
/// Email uniqueness is enforced (case-insensitively): creating a user with an
/// email that already exists returns [`RepoError::AlreadyExists`]. Lookups
/// normalize the email to lowercase, mirroring [`User::new`].
#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn create(&self, user: &User) -> Result<(), RepoError>;
    async fn get(&self, id: UserId) -> Result<User, RepoError>;
    async fn by_email(&self, email: &str) -> Result<User, RepoError>;
}
