//! Typed errors for the inference fallback.

use thiserror::Error;

/// What can go wrong when consulting an inference provider.
///
/// The orchestrator treats every variant as *non-fatal*: a fallback failure
/// degrades to "no suggestion", never to a panic or a wrong import. The
/// deterministic parser is always the fast path and inference only ever adds a
/// hint, so these errors are logged and the caller proceeds without a fallback
/// result rather than failing the pipeline.
#[derive(Debug, Error)]
pub enum LlmError {
    /// The provider could not be reached (network, missing local model server).
    /// Surfaced so the caller can decide to disable the fallback for a while.
    #[error("inference provider unavailable: {0}")]
    Unavailable(String),

    /// The provider returned output that did not satisfy the structured-output
    /// schema. Free-form / malformed model output is rejected, never trusted.
    #[error("model output failed schema validation: {0}")]
    InvalidOutput(String),

    /// The provider returned a transport-level or protocol error.
    #[error("inference request failed: {0}")]
    Request(String),
}
