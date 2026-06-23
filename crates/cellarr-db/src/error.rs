//! The typed error for all persistence operations.
//!
//! Libraries report typed errors via `thiserror`; this is the single error every
//! repository and the [`crate::Database`] handle returns. It wraps the lower-level
//! `sqlx` and serialization failures so callers stay free of those crates.

use thiserror::Error;

/// Errors produced by the persistence layer.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DbError {
    /// A database driver / query error.
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// Applying migrations failed.
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    /// A stored value could not be (de)serialized to/from its JSON column.
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// A stored identifier or timestamp could not be parsed back into its type.
    #[error("malformed stored value in column {column}: {detail}")]
    Decode {
        /// The column whose value was malformed.
        column: &'static str,
        /// Human-readable detail.
        detail: String,
    },

    /// The writer-actor channel is closed (the writer task has stopped).
    #[error("writer task is unavailable: {0}")]
    WriterUnavailable(String),
}

/// Convenience alias for fallible persistence operations.
pub type Result<T> = std::result::Result<T, DbError>;
