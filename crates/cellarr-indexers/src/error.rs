//! Typed errors for indexer integrations.

use thiserror::Error;

/// Failures an indexer adapter can report.
///
/// Network and protocol concerns are kept distinct so callers can decide whether
/// a failure is transient (retry/backoff) or structural (a changed schema, a
/// banned key) — the integration layer is churn-prone by nature, so the decision
/// stage needs to tell those apart (see `docs/06-integrations.md`).
#[derive(Debug, Error)]
pub enum IndexerError {
    /// The HTTP request itself failed (DNS, TLS, connect, timeout, …).
    #[error("http request to indexer failed: {0}")]
    Http(#[from] reqwest::Error),

    /// The indexer answered with a non-success status code.
    ///
    /// `403`/`429` are the load-bearing ones: a key was banned or we are being
    /// rate-limited, which the caller must treat as a temporary ban rather than a
    /// parse failure.
    #[error("indexer returned HTTP {status}{}", .body_snippet.as_deref().map(|b| format!(": {b}")).unwrap_or_default())]
    Status {
        /// The HTTP status code returned.
        status: u16,
        /// A short prefix of the body for diagnostics, if any.
        body_snippet: Option<String>,
    },

    /// The response could not be parsed as the expected XML/JSON shape.
    #[error("failed to parse indexer response: {0}")]
    Parse(String),

    /// A Cardigann definition was malformed or used an unsupported feature.
    #[error("invalid cardigann definition: {0}")]
    Definition(String),

    /// A configured base URL or constructed request URL was invalid.
    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),

    /// The capabilities document did not advertise a search mode the caller asked
    /// for. Per spec we never assume an unadvertised mode is supported.
    #[error("indexer does not advertise search mode '{0}'")]
    UnsupportedMode(String),
}

/// Convenience alias for indexer results.
pub type Result<T> = std::result::Result<T, IndexerError>;
