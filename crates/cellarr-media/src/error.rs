//! Typed errors for the media modules and Identify.
//!
//! Per `docs/agents/conventions.md`, library crates use `thiserror`. The media
//! modules and the Identify mapper are pure transforms over already-fetched
//! metadata and scene mappings, so the failures they describe are *logic*
//! failures: a content node whose coordinates do not belong to its media type,
//! a metadata identity that has not been resolved yet, or a scene mapping that
//! could not place an anime absolute number. I/O failures (HTTP, DB) belong to
//! the adapter crates behind the seam traits, not here.

use cellarr_core::MediaType;

/// Errors produced by a [`crate::module::MovieModule`] /
/// [`crate::module::TvModule`] or by Identify.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MediaError {
    /// A content node was handed to the wrong module, or its coordinates do not
    /// belong to its declared media type.
    #[error("coordinates {coords} are not valid for a {expected:?} module")]
    WrongMediaType {
        /// The media type the module serves.
        expected: MediaType,
        /// A short rendering of the offending coordinates, for the log.
        coords: String,
    },

    /// The content node has no resolved metadata identity (`title_id` / external
    /// id) yet, so the module cannot produce titles, aliases, or naming tokens.
    /// The caller must resolve identity first.
    #[error("content node {0} has no resolved metadata identity yet")]
    UnresolvedIdentity(String),

    /// The metadata payload existed but lacked a field the module needs (e.g. a
    /// series with no title), so the requested derivation is impossible.
    #[error("metadata for {entity} is missing required field `{field}`")]
    IncompleteMetadata {
        /// What the metadata was about (series, movie, …).
        entity: String,
        /// The missing field name.
        field: String,
    },

    /// The scene mapping could not place an anime absolute number onto a
    /// season/episode for the given series. Identify surfaces this rather than
    /// guessing, so the release is routed to manual resolution instead of being
    /// force-fit (the library-safety rule).
    #[error("no scene mapping covers absolute episode {absolute} for series `{series}`")]
    UnmappedAbsolute {
        /// The series the absolute number belongs to.
        series: String,
        /// The absolute episode number that could not be mapped.
        absolute: u32,
    },

    /// The scene-mapping payload from the provider was malformed (not the shape
    /// Identify expects). Distinct from `UnmappedAbsolute`: there the mapping is
    /// well-formed but simply does not cover the number.
    #[error("scene mapping for series `{series}` is malformed: {detail}")]
    MalformedSceneMapping {
        /// The series the mapping was for.
        series: String,
        /// What was wrong with it.
        detail: String,
    },
}
