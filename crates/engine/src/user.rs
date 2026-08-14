use chrono::NaiveDate;

use crate::ids::UserId;

/// A registered user who owns portfolios.
///
/// Deliberately **not** serde-enabled: `password_hash` is credential material
/// and must never leave the backend. The API shapes its own user DTO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: UserId,
    /// Lowercase-normalized email address (uniqueness key).
    pub email: String,
    /// Argon2id password hash (PHC string).
    pub password_hash: String,
    /// Date the account was created.
    pub created_at: NaiveDate,
    /// Date the account record was last updated.
    pub updated_at: NaiveDate,
}

impl User {
    /// Creates a user; `email` is normalized to lowercase so uniqueness and
    /// lookups are case-insensitive. `as_of` stamps `created_at`/`updated_at`.
    pub fn new(
        id: UserId,
        email: impl Into<String>,
        password_hash: impl Into<String>,
        as_of: NaiveDate,
    ) -> Self {
        Self {
            id,
            email: email.into().to_lowercase(),
            password_hash: password_hash.into(),
            created_at: as_of,
            updated_at: as_of,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_is_normalized_to_lowercase() {
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let u = User::new(UserId::new(), "Alice@Example.COM", "hash", d);
        assert_eq!(u.email, "alice@example.com");
    }
}
