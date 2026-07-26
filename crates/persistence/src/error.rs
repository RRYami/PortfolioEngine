//! Mapping from `sqlx` errors to the repository error contract.

use ptf_engine::RepoError;

/// Maps a [`sqlx::Error`] to the repository error contract.
///
/// - `RowNotFound` and foreign-key violations (`23503`, e.g. appending a
///   transaction to a missing portfolio) become [`RepoError::NotFound`].
/// - Unique violations (`23505`) become [`RepoError::AlreadyExists`].
/// - Everything else becomes [`RepoError::Database`].
pub fn map_sqlx(err: sqlx::Error) -> RepoError {
    match err {
        sqlx::Error::RowNotFound => RepoError::NotFound,
        sqlx::Error::Database(db_err) => match db_err.code().as_deref() {
            // unique_violation
            Some("23505") => RepoError::AlreadyExists(db_err.message().to_string()),
            // foreign_key_violation
            Some("23503") => RepoError::NotFound,
            _ => RepoError::Database(Box::new(sqlx::Error::Database(db_err))),
        },
        other => RepoError::Database(Box::new(other)),
    }
}

/// Maps a domain error raised while reconstituting rows (e.g. an invalid
/// currency code in storage) to [`RepoError::Serialization`].
pub fn map_domain(err: &ptf_engine::DomainError) -> RepoError {
    RepoError::Serialization(err.to_string())
}
