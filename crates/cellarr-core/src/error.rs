//! Typed error definitions for the core domain.
//!
//! Core is pure (no I/O), so these errors describe *logic* failures: illegal
//! pipeline transitions, malformed coordinates for a media type, and the like.
//! I/O-bearing crates define their own error types and may wrap these.

use crate::media::MediaType;
use crate::pipeline::Stage;

/// Errors produced by pure core logic.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CoreError {
    /// A pipeline transition was requested that the state machine forbids.
    #[error("illegal pipeline transition from {from:?} to {to:?}")]
    IllegalTransition {
        /// The stage the item was in.
        from: Stage,
        /// The stage that was illegally requested.
        to: Stage,
    },

    /// Coordinates did not match the media type they were paired with
    /// (e.g. `Coordinates::Track` on a movie).
    #[error("coordinates are invalid for media type {media_type:?}: {detail}")]
    InvalidCoordinates {
        /// The media type the coordinates were applied to.
        media_type: MediaType,
        /// Human-readable explanation of the mismatch.
        detail: String,
    },

    /// A value failed an invariant check that is not specific to transitions
    /// or coordinates.
    #[error("invariant violated: {0}")]
    Invariant(String),
}

/// Convenience alias for fallible core operations.
pub type Result<T> = std::result::Result<T, CoreError>;
