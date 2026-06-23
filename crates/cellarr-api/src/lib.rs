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
pub mod tags;

use axum::Router;
use tower_http::trace::TraceLayer;

pub use auth::AuthConfig;
pub use error::{ApiError, ApiResult};
pub use events::{DomainEvent, EventBus};
pub use state::AppState;

/// Build the complete application router.
///
/// Surfaces:
/// - `/api/v1` — the native cellarr REST + SSE API;
/// - `/api/v3` — cellarr's own v3 face (app surface auto-selected per library);
/// - `/sonarr/api/v3` — the **Sonarr face** (TV resources, Sonarr v4 identity);
/// - `/radarr/api/v3` — the **Radarr face** (movie resources, Radarr v5 identity);
/// - everything else — the embedded SPA assets.
///
/// A real stack adds cellarr **twice**: as a Sonarr at `…/sonarr` and a Radarr
/// at `…/radarr`. The two faces share one handler core; only `appName`/version
/// and which media type's list resources are exposed differ.
///
/// Each v3 mount owns a 404-JSON fallback, so an unknown `/api/v3/*` (or
/// `/sonarr|radarr/api/v3/*`) path returns structured JSON — **not** the SPA
/// HTML (bug B1). Only genuinely non-API paths reach the asset fallback.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .nest("/api/v1", native::router(state.clone()))
        .nest("/api/v3", shim::router(state.clone(), shim::Face::Cellarr))
        .nest(
            "/sonarr/api/v3",
            shim::router(state.clone(), shim::Face::Sonarr),
        )
        .nest("/radarr/api/v3", shim::router(state, shim::Face::Radarr))
        // Only non-API paths fall through to the embedded frontend; the v3
        // mounts return their own 404 JSON for unknown API paths.
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
