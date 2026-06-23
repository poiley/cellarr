//! The metadata-lookup seam the `/api/v3` shim reads identities from.
//!
//! The shim's `series/lookup` and `movie/lookup` (and the `series`/`movie` list
//! resources) must answer with **real** identities — a human title and the
//! external id the ecosystem keys on (`tvdbId` for Sonarr, `tmdbId` for Radarr) —
//! not the search term echoed back or a bare UUID (the Phase A deferred gap).
//!
//! Those identities come from `cellarr-meta` (TheTVDB / TMDb). But the API crate
//! must not depend on a specific source crate (it stays free of every provider's
//! schema, like core). So the shim depends on this thin, object-safe
//! [`MetadataLookup`] seam; the wiring crate (`cellarr-cli`) implements it over
//! the live `cellarr-meta` sources and injects it via [`AppState`].
//!
//! # Graceful degradation
//!
//! A lookup that has no configured source (e.g. movies with no TMDb key) returns
//! [`LookupOutcome::Unavailable`] with a clear reason — **never** an error that
//! would 500 the daemon. Offline is non-negotiable: the shim renders that as an
//! empty, clearly-flagged result so a client (Overseerr) degrades rather than
//! breaking.

use async_trait::async_trait;

use cellarr_core::MediaType;

/// One resolved identity candidate from a metadata source.
///
/// The fields the v3 lookup/list resources surface: a stable source id, a human
/// title, an optional year, and the cross-referenced external ids (so the shim
/// can pull out `tvdbId`/`tmdbId`/`imdbId`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupCandidate {
    /// The source-native id (a TVDB or TMDb numeric id, as a string).
    pub source_id: String,
    /// The media type this candidate is.
    pub media_type: MediaType,
    /// Human display title (never a UUID / never the echoed search term).
    pub title: String,
    /// Release / first-air year, when the source provides one.
    pub year: Option<u16>,
    /// Short overview/synopsis, when present.
    pub overview: Option<String>,
    /// Cross-referenced external ids as `(scheme, value)` pairs, e.g.
    /// `("tvdb", "81189")`, `("imdb", "tt0903747")`, `("tmdb", "603")`.
    pub external_ids: Vec<(String, String)>,
}

impl LookupCandidate {
    /// The value of an external id by scheme (case-insensitive), if present.
    #[must_use]
    pub fn external_id(&self, scheme: &str) -> Option<&str> {
        self.external_ids
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(scheme))
            .map(|(_, v)| v.as_str())
    }
}

/// The outcome of a metadata lookup.
///
/// Distinguishing "no source configured" ([`Unavailable`](Self::Unavailable))
/// from "source ran, found nothing" (`Resolved(vec![])`) lets the shim degrade
/// gracefully and report *why* a movie lookup is empty (no TMDb key) without
/// erroring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupOutcome {
    /// The source ran and returned these candidates (possibly empty).
    Resolved(Vec<LookupCandidate>),
    /// No source is configured/reachable for this media type. The reason is a
    /// short, non-secret human string (e.g. "no TMDb API key configured").
    Unavailable(String),
}

/// The object-safe metadata-lookup seam the shim depends on.
///
/// Implemented by the wiring crate over the live `cellarr-meta` sources; held in
/// [`AppState`] as `Option<Arc<dyn MetadataLookup>>`. `None` means no metadata
/// wiring at all (the shim then reports every lookup as unavailable).
#[async_trait]
pub trait MetadataLookup: Send + Sync {
    /// Search the source for `media_type` by free-text `term`.
    ///
    /// Returns [`LookupOutcome::Unavailable`] (not `Err`) when no source is
    /// configured for that media type, so the caller degrades gracefully.
    /// `Err` is reserved for a configured source that genuinely failed mid-call
    /// (transport/decode), which the shim maps to a 502-style structured error.
    async fn search(
        &self,
        media_type: MediaType,
        term: &str,
    ) -> Result<LookupOutcome, MetadataLookupError>;
}

/// A failure from a *configured* metadata source (transport/decode/HTTP).
///
/// A missing credential is **not** this — that is [`LookupOutcome::Unavailable`].
/// This is only for a source that was supposed to answer and could not.
#[derive(Debug, thiserror::Error)]
#[error("metadata lookup failed for {provider}: {detail}")]
pub struct MetadataLookupError {
    /// The source that failed (e.g. `thetvdb`).
    pub provider: String,
    /// A short, non-secret description of the failure.
    pub detail: String,
}
