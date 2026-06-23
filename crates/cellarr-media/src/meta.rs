//! Typed metadata views and the lookup seam the modules read from.
//!
//! `cellarr_core::MetadataSource` deliberately returns opaque `serde_json::Value`
//! so core stays free of every provider's schema (see `docs/07-metadata-service.md`).
//! `cellarr-media` is where that opaque payload becomes typed for *its* use: the
//! modules need a series/movie title, aliases, and external ids to build search
//! terms and naming tokens, nothing more.
//!
//! Rather than scatter `value["title"].as_str()` through the modules, the
//! provider-shaped facts the modules consume are named here as small structs,
//! and the modules depend on the [`MetadataLookup`] trait — a thin seam over a
//! `MetadataSource` (or any mock). This keeps the modules pure and lets tests
//! supply fixtures without a live `cellarr-meta`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use cellarr_core::{ContentId, TitleId};

/// The movie facts a [`crate::module::MovieModule`] needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovieMeta {
    /// Primary title.
    pub title: String,
    /// Alternative titles (other-language, AKA), most useful first.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Release year, when known (disambiguates same-named films).
    #[serde(default)]
    pub year: Option<u16>,
    /// External ids as `(key, value)` pairs (e.g. `("imdbid", "tt0133093")`,
    /// `("tmdbid", "603")`). Kept as pairs because indexers key on these
    /// directly in their query string.
    #[serde(default)]
    pub external_ids: Vec<(String, String)>,
}

/// The series facts a [`crate::module::TvModule`] needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesMeta {
    /// Primary series title.
    pub title: String,
    /// Alternative/scene titles. Anime in particular is searched under several
    /// romanizations, so this list matters for search recall.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Series start year, when known.
    #[serde(default)]
    pub year: Option<u16>,
    /// External ids (e.g. `("tvdbid", "81797")`, `("anidbid", "23")`).
    #[serde(default)]
    pub external_ids: Vec<(String, String)>,
}

/// The lookup seam the modules depend on.
///
/// A real implementation wraps a `cellarr_core::MetadataSource` and deserializes
/// its opaque JSON into the views above; tests supply an in-memory map. Either
/// way the modules never see provider-specific JSON, and never do I/O directly.
#[async_trait]
pub trait MetadataLookup: Send + Sync {
    /// The typed error this lookup reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Resolve the movie identity for a content node, if one has been linked.
    async fn movie_meta(
        &self,
        content: ContentId,
        title_id: Option<TitleId>,
    ) -> Result<Option<MovieMeta>, Self::Error>;

    /// Resolve the series identity that a TV content node belongs to.
    ///
    /// `title_id` is the resolved identity link from the node (or its series
    /// ancestor); `None` means identity is unresolved.
    async fn series_meta(
        &self,
        content: ContentId,
        title_id: Option<TitleId>,
    ) -> Result<Option<SeriesMeta>, Self::Error>;
}
