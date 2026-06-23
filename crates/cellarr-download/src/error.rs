//! The typed error every download adapter reports.
//!
//! Libraries use `thiserror` (see `docs/agents/conventions.md`); the binary maps
//! these to pipeline failure transitions. The variants are chosen so the caller
//! can decide *what to do*: an [`DownloadError::Auth`] means re-login/notify, a
//! [`DownloadError::NotFound`] means the download vanished (re-search), a
//! [`DownloadError::DownloadFailed`] means blocklist + re-search, while
//! [`DownloadError::Transport`] / [`DownloadError::Api`] are retryable client
//! faults.

use thiserror::Error;

/// Errors a [`crate::DownloadClient`] adapter can return.
#[derive(Debug, Error)]
pub enum DownloadError {
    /// Authentication failed (bad credentials, expired/rejected session). The
    /// caller should surface this rather than treat the download as failed.
    #[error("download client authentication failed: {0}")]
    Auth(String),

    /// The requested download id is not known to the client (it was removed out
    /// of band, or never existed).
    #[error("download {0} not found on client")]
    NotFound(String),

    /// The download itself failed on the client (torrent errored, Usenet repair
    /// or unpack failed). Terminal for this release; the caller re-searches.
    #[error("download failed on client: {0}")]
    DownloadFailed(String),

    /// The client accepted the request but reported an application-level error
    /// (e.g. SABnzbd `{"status": false, "error": ...}`).
    #[error("download client API error: {0}")]
    Api(String),

    /// The client returned a response the adapter could not understand.
    #[error("unexpected response from download client: {0}")]
    UnexpectedResponse(String),

    /// A transport-level failure (connection refused, TLS, timeout).
    #[error("transport error: {0}")]
    Transport(String),

    /// The adapter was misconfigured (missing host, bad settings JSON).
    #[error("download client misconfigured: {0}")]
    Config(String),
}
