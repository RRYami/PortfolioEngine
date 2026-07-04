use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use ptf_engine::{DomainError, FxError, PriceError, RepoError, RiskError, ValuationError};
use serde_json::json;

/// Error type that maps cleanly onto HTTP responses.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

impl From<RepoError> for ApiError {
    fn from(e: RepoError) -> Self {
        match e {
            RepoError::NotFound => ApiError::NotFound,
            RepoError::AlreadyExists(s) | RepoError::Conflict(s) => ApiError::BadRequest(s),
            other => ApiError::Internal(other.to_string()),
        }
    }
}

impl From<DomainError> for ApiError {
    fn from(e: DomainError) -> Self {
        ApiError::BadRequest(e.to_string())
    }
}

impl From<RiskError> for ApiError {
    fn from(e: RiskError) -> Self {
        // Insufficient history / singular covariance are user-fixable inputs.
        ApiError::BadRequest(e.to_string())
    }
}

impl From<ValuationError> for ApiError {
    fn from(e: ValuationError) -> Self {
        ApiError::BadRequest(e.to_string())
    }
}

impl From<PriceError> for ApiError {
    fn from(e: PriceError) -> Self {
        ApiError::BadRequest(e.to_string())
    }
}

impl From<FxError> for ApiError {
    fn from(e: FxError) -> Self {
        ApiError::BadRequest(e.to_string())
    }
}
