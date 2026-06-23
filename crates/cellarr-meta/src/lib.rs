//! cellarr-meta — the metadata service (the Skyhook rebuild).
//!
//! Normalizes identity/descriptive data from external sources into one clean
//! schema the rest of cellarr consumes, and provides scene-mapping data for
//! anime numbering. It can run **embedded** in the daemon or as a **standalone,
//! self-hostable** binary (`meta-service/`, feature `standalone`). See
//! `docs/specs/cellarr-meta.md` and `docs/07-metadata-service.md`.
//!
//! # Shape
//!
//! - Each source is an adapter implementing [`cellarr_core::MetadataSource`]
//!   ([`TmdbSource`] for movies, [`TheTvdbSource`] for TV in v1) over an injected
//!   [`http::Fetcher`] — live ([`http::ReqwestFetcher`]) or recorded
//!   ([`http::RecordedFetcher`]) so **no live source touches the test path**.
//! - Every adapter sits behind a per-source [`ratelimit::RateLimiter`]
//!   (conservative, `governor`) and a [`cache::MetaCache`] (in-process `moka`,
//!   per-source TTL, stampede-protected). A persisted cache table via
//!   `cellarr-db` is a deliberate follow-up — this crate does **not** depend on
//!   the database.
//! - Adapters normalize provider JSON into [`normalized::Metadata`] /
//!   [`normalized::SearchResult`]; consumers wanting raw payloads use the trait's
//!   `serde_json::Value` form.
//! - The [`scene`] module parses TheXEM + anime-lists shapes into a neutral
//!   [`scene::SceneMap`] and remaps [`cellarr_core::Coordinates::Absolute`] to a
//!   canonical episode.
//!
//! # Graceful degradation
//!
//! Bring-your-own-key: with no key configured a source reports
//! [`MetaError::NoCredential`] rather than failing the daemon; offline is a
//! non-negotiable, so callers treat an unreachable source as "unavailable" and
//! carry on.
//!
//! # Licensing landmines (surfaced, not hidden)
//!
//! TMDb forbids commercial use without a separate agreement and TheTVDB is paid;
//! both are flagged in `docs/07-metadata-service.md`. We never proxy through the
//! originals' Skyhook/RadarrAPI (their ToS).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cache;
pub mod config;
pub mod error;
pub mod http;
pub mod normalized;
pub mod ratelimit;
pub mod scene;
pub mod thetvdb;
pub mod tmdb;

pub use config::{TheTvdbConfig, TmdbConfig};
pub use error::{MetaError, Result};
pub use http::{Fetcher, HttpResponse, RecordedFetcher, ReqwestFetcher};
pub use normalized::{ChildNode, Image, Metadata, SearchResult};
pub use scene::{parse_anime_list_entry, parse_xem, SceneMap, SceneRule};
pub use thetvdb::TheTvdbSource;
pub use tmdb::TmdbSource;

#[cfg(feature = "standalone")]
pub mod server;

/// Run the metadata service as a standalone HTTP server.
///
/// A thin self-host wrapper exposing the same normalized surface over HTTP, for
/// privacy-minded users who run their own instance (`docs/07-metadata-service.md`).
/// Binds to `CELLARR_META_ADDR` when set, otherwise `127.0.0.1:7878`.
///
/// # Errors
/// Propagates bind/serve failures from the underlying server.
#[cfg(feature = "standalone")]
pub async fn serve_standalone() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let addr: std::net::SocketAddr = std::env::var("CELLARR_META_ADDR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| ([127, 0, 0, 1], 7878).into());
    server::serve(addr).await
}
