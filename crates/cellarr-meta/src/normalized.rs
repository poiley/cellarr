//! The normalized metadata schema.
//!
//! The whole point of the metadata service (the Skyhook rebuild, see
//! `docs/07-metadata-service.md`) is that the rest of cellarr consumes **one**
//! clean schema regardless of which source produced it. Each source adapter
//! parses its provider-specific JSON and emits these types; `cellarr-media`
//! never has to know whether a title came from TMDb or TheTVDB.
//!
//! These are deliberately small and source-agnostic. Rich provider-only fields
//! are dropped here on purpose — if a consumer needs them it can ask for the raw
//! payload via the [`MetadataSource`] trait, which returns `serde_json::Value`.
//!
//! [`MetadataSource`]: cellarr_core::MetadataSource

use cellarr_core::MediaType;
use serde::{Deserialize, Serialize};

/// A candidate identity returned by a search.
///
/// Enough to disambiguate and then fetch: a stable source id, a display title,
/// an optional year, and any cross-referenced external ids (IMDb, TVDB, TMDb).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    /// The source-native id used to [`fetch`](cellarr_core::MetadataSource::fetch).
    pub source_id: String,
    /// The media type this candidate is.
    pub media_type: MediaType,
    /// Display title.
    pub title: String,
    /// Release/first-air year, when the source provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    /// A short overview/synopsis, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    /// Cross-referenced external ids, as `(scheme, value)` (e.g.
    /// `("imdb", "tt0903747")`, `("tvdb", "81189")`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_ids: Vec<(String, String)>,
}

/// An image reference (poster, banner, fanart, still).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    /// The kind of artwork (`poster`, `banner`, `fanart`, `still`, …).
    pub kind: String,
    /// A resolvable URL, or a source-relative path the caller can compose.
    pub url: String,
}

/// A normalized child node (season/episode for TV; unused for flat movies).
///
/// This is the "child structure" the `fetch` contract promises. For TV, a series
/// fetch yields seasons each yielding episodes; the numbering here maps directly
/// onto [`cellarr_core::Coordinates`] when `cellarr-media` builds the content
/// tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildNode {
    /// The source-native id of this child, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// Season number (0 = specials), when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    /// Episode number within the season, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    /// Absolute episode number across the series (anime numbering), when the
    /// source carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute: Option<u32>,
    /// Air date in ISO `yyyy-mm-dd` form, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
    /// The child's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// The full normalized metadata record for an identity.
///
/// The common schema returned to `cellarr-media` from any source's `fetch`.
// No `Eq`: `rating` is an `f32` (PartialEq only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    /// The source-native id.
    pub source_id: String,
    /// The media type.
    pub media_type: MediaType,
    /// Canonical title.
    pub title: String,
    /// Release/first-air year, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    /// Overview/synopsis, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    /// Runtime in minutes (a movie's running time), when the source provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<u32>,
    /// Theatrical/physical release date in ISO `yyyy-mm-dd` form, when known
    /// (movies). For a series this is the first-air date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    /// Digital (home/streaming) release date in ISO `yyyy-mm-dd` form, when the
    /// source distinguishes it from the theatrical release (movies only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digital_release: Option<String>,
    /// Cross-referenced external ids, as `(scheme, value)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_ids: Vec<(String, String)>,
    /// Child structure (seasons/episodes for TV; empty for flat movies).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ChildNode>,
    /// Artwork references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
    /// Genres (e.g. `["Animation", "Comedy"]`), most significant first, when known.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    /// Primary user rating on a 0–10 scale (TMDB `vote_average`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<f32>,
    /// Number of votes backing `rating` (TMDB `vote_count`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating_votes: Option<u32>,
}
