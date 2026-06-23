//! The job/runner error type.
//!
//! The runner orchestrates many crates, each with its own typed error. Rather
//! than leak every dependency's error into the public surface, those are erased
//! into a single boxed source ([`JobError::Stage`]) carrying the stage that
//! failed, so the pipeline can record *where* it failed in the decision log
//! without core depending on any I/O crate. Persistence and scheduling failures
//! keep dedicated variants because callers branch on them.

use cellarr_core::pipeline::Stage;

/// A type-erased source error from a delegated crate.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Errors raised while scheduling or executing pipeline work.
#[derive(Debug, thiserror::Error)]
pub enum JobError {
    /// A pipeline stage failed. The `stage` is carried so the failure can be
    /// logged against the exact transition that broke (grab-failed,
    /// import-failed, …) rather than as an opaque error.
    #[error("pipeline stage {stage:?} failed: {source}")]
    Stage {
        /// The stage that was executing when the failure occurred.
        stage: Stage,
        /// The underlying cause.
        #[source]
        source: BoxError,
    },

    /// An illegal stage transition was requested — a programming error in the
    /// runner, surfaced rather than panicked.
    #[error(transparent)]
    Transition(#[from] cellarr_core::CoreError),

    /// A persistence operation (job store, repository) failed.
    #[error("persistence failed: {0}")]
    Persistence(#[source] BoxError),

    /// No download client / indexer / media module was configured for the work.
    #[error("no {resource} configured for {detail}")]
    NotConfigured {
        /// The kind of missing resource.
        resource: &'static str,
        /// What was being attempted.
        detail: String,
    },

    /// A job was cancelled before it could complete.
    #[error("job {0} was cancelled")]
    Cancelled(String),
}

impl JobError {
    /// Build a [`JobError::Stage`] from any error, tagging it with `stage`.
    pub fn stage<E>(stage: Stage, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Stage {
            stage,
            source: Box::new(source),
        }
    }

    /// Build a [`JobError::Stage`] from an already-boxed error.
    #[must_use]
    pub fn stage_boxed(stage: Stage, source: BoxError) -> Self {
        Self::Stage { stage, source }
    }
}

/// Convenience result alias for the crate.
pub type Result<T> = std::result::Result<T, JobError>;
