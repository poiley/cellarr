//! Typed errors for the importer.
//!
//! Libraries use `thiserror` (conventions.md). Every fallible path that can be
//! reached from a user's on-disk database returns one of these rather than
//! panicking: a malformed source DB is *expected* input, not a bug.

use thiserror::Error;

/// Errors raised while previewing or importing an existing *arr install.
#[derive(Debug, Error)]
pub enum MigrationError {
    /// The source SQLite database could not be opened or queried.
    #[error("source database error: {0}")]
    Source(#[from] sqlx::Error),

    /// The source database did not look like any *arr schema we recognize.
    ///
    /// Carries the path so a guided import can tell the user *which* file failed.
    #[error("could not detect a Sonarr or Radarr database at {path}: {detail}")]
    Unrecognized {
        /// The path that was probed.
        path: String,
        /// Why detection failed (e.g. which marker tables were absent).
        detail: String,
    },

    /// A JSON column in the source (quality, profile items, CF specs) was
    /// malformed. Carries context so the offending row is identifiable.
    #[error("malformed JSON in {context}: {source}")]
    Json {
        /// What was being decoded (e.g. "Radarr MovieFile.Quality").
        context: String,
        /// The underlying serde error.
        source: serde_json::Error,
    },

    /// Mapping a source custom format into cellarr's model failed.
    #[error("custom-format mapping failed: {0}")]
    CustomFormat(#[from] cellarr_decide::DecideError),

    /// Writing the mapped data into the destination cellarr DB failed.
    #[error("destination database error: {0}")]
    Destination(#[from] cellarr_db::DbError),
}

/// Convenience result alias for the crate.
pub type Result<T> = std::result::Result<T, MigrationError>;
