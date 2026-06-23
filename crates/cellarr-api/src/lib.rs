//! cellarr-api — the HTTP API.
//!
//! Serves three surfaces from the one binary (docs/09-api.md):
//!
//! 1. the **native** versioned REST + SSE API under `/api/v1`
//!    ([`native`]/[`stream`]),
//! 2. the **`/api/v3` Radarr/Sonarr compatibility shim** ([`shim`]) — an
//!    external contract the ecosystem depends on, and
//! 3. the embedded **SRCL frontend** assets ([`assets`]).
//!
//! Dependencies are injected via [`AppState`]; reads go through `cellarr-db`
//! repositories and commands through the `cellarr-jobs` scheduler. Errors are
//! structured ([`ApiError`]) with a stable `code`. Live updates are pushed on
//! real domain transitions through the [`events::EventBus`], not a polling timer.

#![forbid(unsafe_code)]

pub mod assets;
pub mod auth;
pub mod commands;
pub mod error;
pub mod events;
pub mod native;
pub mod openapi;
pub mod shim;
pub mod state;
pub mod stream;

use axum::Router;
use tower_http::trace::TraceLayer;

pub use auth::AuthConfig;
pub use error::{ApiError, ApiResult};
pub use events::{DomainEvent, EventBus};
pub use state::AppState;

/// Build the complete application router: native API, the v3 shim, and the
/// embedded-asset fallback. The asset handler is the fallback so any
/// non-API path serves the SPA (or the "UI not built yet" placeholder).
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .nest("/api/v1", native::router(state.clone()))
        .nest("/api/v3", shim::router(state))
        // Any unmatched path falls through to the embedded frontend.
        .fallback(assets::serve)
        .layer(TraceLayer::new_for_http())
}

/// Serve the API on an already-bound listener until the process is stopped.
///
/// Binding is the caller's responsibility (tests bind `127.0.0.1:0`; the daemon
/// binds its configured address) so this crate never assumes a fixed port.
///
/// # Errors
/// Returns any error from the underlying `axum::serve`.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    axum::serve(listener, build_router(state).into_make_service()).await
}

/// Serve the API until `shutdown` resolves, then stop accepting and let in-flight
/// requests finish (axum's graceful shutdown). The daemon drives this with its
/// signal future so a clean stop drains the server before the database is closed.
///
/// # Errors
/// Returns any error from the underlying `axum::serve`.
pub async fn serve_with_shutdown<F>(
    listener: tokio::net::TcpListener,
    state: AppState,
    shutdown: F,
) -> std::io::Result<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    axum::serve(listener, build_router(state).into_make_service())
        .with_graceful_shutdown(shutdown)
        .await
}
