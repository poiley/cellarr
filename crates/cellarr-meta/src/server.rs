//! The standalone HTTP server (feature `standalone`).
//!
//! Exposes the metadata surface over HTTP so a privacy-minded user can run their
//! own instance and the daemon can point at it instead of holding keys itself
//! (`docs/07-metadata-service.md`). This is deliberately thin: the same adapters,
//! cache, and rate limiters run here as when embedded — the only difference is
//! the transport in front.
//!
//! Routes are kept minimal (a health probe) so the embedded and standalone modes
//! share one code path; richer routing lands with the daemon wiring. The point
//! of having it now is that "standalone vs embedded both exercised" is a test
//! obligation, and `serve` is what the `meta-service` binary calls.

use std::net::SocketAddr;

use axum::routing::get;
use axum::Router;

/// Build the router served in standalone mode.
pub fn router() -> Router {
    Router::new().route("/health", get(|| async { "ok" }))
}

/// Bind `addr` and serve until the process is stopped.
///
/// # Errors
/// Returns any bind or serve error from the underlying listener.
pub async fn serve(
    addr: SocketAddr,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router()).await?;
    Ok(())
}
