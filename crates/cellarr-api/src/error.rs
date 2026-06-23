//! Structured API errors.
//!
//! Every fallible handler returns [`ApiError`], which serializes to a stable
//! JSON body `{ "code": "...", "message": "..." }` (docs/09-api.md). Clients
//! branch on the machine-readable `code`, not the HTTP status, so the string
//! codes here are part of the contract and must stay stable.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// A handler error with a stable machine-readable code and a human message.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The request was malformed (bad path/body/query).
    #[error("{0}")]
    BadRequest(String),
    /// Authentication is missing or invalid on a mutating endpoint.
    #[error("missing or invalid API key")]
    Unauthorized,
    /// The addressed resource does not exist.
    #[error("{0}")]
    NotFound(String),
    /// A persistence-layer failure. The underlying detail is logged, never
    /// returned, so we never leak SQL or secrets to a client.
    #[error("database error")]
    Db(#[from] cellarr_db::DbError),
    /// A domain-rule violation surfaced from `cellarr-core`.
    #[error("{0}")]
    Domain(String),
    /// A failure submitting a command to the job scheduler.
    #[error("{0}")]
    Command(String),
    /// A configured upstream (a metadata source) was reachable but failed the
    /// request (transport/decode/HTTP). Distinct from [`Internal`](Self::Internal)
    /// so a client can tell "the external source is misbehaving" from "cellarr
    /// has a bug". Detail is logged; a generic message is returned.
    #[error("upstream metadata source error")]
    Upstream(String),
    /// Anything unanticipated. Detail is logged, not returned.
    #[error("internal error")]
    Internal(String),
}

impl ApiError {
    /// The stable string code for this error. Part of the wire contract.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            ApiError::BadRequest(_) => "bad_request",
            ApiError::Unauthorized => "unauthorized",
            ApiError::NotFound(_) => "not_found",
            ApiError::Db(_) => "db_error",
            ApiError::Domain(_) => "domain_error",
            ApiError::Command(_) => "command_failed",
            ApiError::Upstream(_) => "upstream_error",
            ApiError::Internal(_) => "internal_error",
        }
    }

    /// The HTTP status that accompanies the structured body.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) | ApiError::Domain(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Upstream(_) => StatusCode::BAD_GATEWAY,
            ApiError::Db(_) | ApiError::Command(_) | ApiError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

/// The JSON body shape for an error response.
#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Log server-side faults with their internal detail; the client only
        // ever sees the generic message so secrets/SQL never leak.
        match &self {
            ApiError::Db(e) => tracing::error!(error = %e, "database error serving request"),
            ApiError::Internal(detail) => tracing::error!(detail, "internal error serving request"),
            ApiError::Command(detail) => tracing::warn!(detail, "command submission failed"),
            ApiError::Upstream(detail) => {
                tracing::warn!(detail, "upstream metadata source error serving request")
            }
            _ => {}
        }
        let body = ErrorBody {
            code: self.code(),
            message: self.to_string(),
        };
        (self.status(), Json(body)).into_response()
    }
}

/// Convenience alias for handler results.
pub type ApiResult<T> = Result<T, ApiError>;
