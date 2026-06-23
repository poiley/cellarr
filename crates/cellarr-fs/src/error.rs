//! Typed errors for library file operations.
//!
//! Every fallible path in this crate returns [`FsError`]. The variants are
//! deliberately fine-grained because callers (the pipeline, the UI) surface them
//! to users and branch on them — an import held for review reads very
//! differently from a genuine I/O failure.

use std::path::PathBuf;

/// An error from a library file operation.
///
/// Variants distinguish *planning* faults (caught at Stage/Verify, before any
/// mutation) from *commit* faults (which can leave durable state and must be
/// recoverable). The safety discipline depends on never confusing the two.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    /// A path that must exist did not, or could not be inspected.
    #[error("path does not exist or is not accessible: {path}")]
    MissingPath {
        /// The offending path.
        path: PathBuf,
    },

    /// A destination already holds a file that the plan did not mark as replaced.
    ///
    /// Refusing to clobber an unexpected file is a core safety property: the
    /// planner must account for everything it would overwrite.
    #[error("destination already exists and is not a planned replacement: {path}")]
    UnexpectedDestination {
        /// The destination that was already occupied.
        path: PathBuf,
    },

    /// Verify found the actual file no longer matches what the plan assumed
    /// (re-parse disagreement, wrong size, missing source).
    #[error("verification failed for {path}: {detail}")]
    VerificationFailed {
        /// The file that failed verification.
        path: PathBuf,
        /// Why it failed.
        detail: String,
    },

    /// The destination filesystem does not have room for the file.
    #[error("insufficient space at {path}: need {needed} bytes, have {available}")]
    InsufficientSpace {
        /// The destination directory.
        path: PathBuf,
        /// Bytes required.
        needed: u64,
        /// Bytes available.
        available: u64,
    },

    /// A naming token required by the format string was not supplied.
    #[error("naming token {token:?} referenced by the format is missing")]
    MissingToken {
        /// The token name that the format referenced but the module did not
        /// supply.
        token: String,
    },

    /// The rendered name was empty or otherwise unusable.
    #[error("rendered an invalid name: {detail}")]
    InvalidName {
        /// What was wrong with the rendered name.
        detail: String,
    },

    /// An underlying filesystem operation failed. The path it was operating on
    /// is attached so failures are diagnosable without a backtrace.
    #[error("io error on {path}: {source}")]
    Io {
        /// The path being operated on when the error occurred.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// A blocking task that performs the I/O panicked or was cancelled.
    #[error("background file task failed to complete: {0}")]
    TaskJoin(String),
}

impl FsError {
    /// Attach a path to a raw [`std::io::Error`].
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        FsError::Io {
            path: path.into(),
            source,
        }
    }
}

/// Convenience alias for results in this crate.
pub type Result<T> = std::result::Result<T, FsError>;
