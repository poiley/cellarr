//! cellarr-meta — the metadata service (stub).
//!
//! [`cellarr_core::MetadataSource`] implementations (TMDb, TheTVDB, MusicBrainz,
//! OpenLibrary, AniDB) with caching and per-source rate limits; optionally
//! exposes an axum endpoint when run standalone (feature `standalone`). Not yet
//! implemented; this stub exists so the workspace resolves. Real work lands per
//! `docs/specs/cellarr-meta.md` and `docs/07-metadata-service.md`.

#![forbid(unsafe_code)]

/// Run the metadata service as a standalone HTTP server.
///
/// Placeholder until the service is implemented; returns immediately so the
/// `meta-service` binary links and the workspace resolves.
#[cfg(feature = "standalone")]
pub async fn serve_standalone() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Ok(())
}
