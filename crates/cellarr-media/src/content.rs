//! The content-lookup seam the modules use to resolve matches.
//!
//! `MediaModule::match_release` is handed only a `ParsedRelease`; to answer
//! *which content node(s)* it satisfies, a module must consult the library. It
//! does so through this trait rather than a `ContentRepository` directly, because
//! matching is a title/coordinate query (not a key lookup) and core's repository
//! seam intentionally stays minimal. A real implementation in `cellarr-db`/the
//! pipeline backs this with an indexed search; tests supply an in-memory set.

use async_trait::async_trait;

use cellarr_core::{ContentRef, MediaType};

/// A library node a parse might match, with the title it is known by.
///
/// Carries the searchable `title` alongside the [`ContentRef`] so a module can
/// compare a parse's clean title against the library without a second metadata
/// round-trip. `aliases` lets anime/scene names match too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentCandidate {
    /// The node reference handed back in a [`cellarr_core::ContentMatch`].
    pub content_ref: ContentRef,
    /// The node's primary title (series/movie title), for title matching.
    pub title: String,
    /// Alternative titles the node is also known by.
    pub aliases: Vec<String>,
}

/// The lookup seam `match_release` uses to find candidate nodes.
#[async_trait]
pub trait ContentLookup: Send + Sync {
    /// The typed error this lookup reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// All monitored candidate nodes of `media_type` whose title or an alias is
    /// a plausible match for `title_query` (the implementation decides how loose
    /// the match is; the module re-checks coordinates and refines confidence).
    ///
    /// For TV this returns the *episode* nodes (so a single parse can fan out to
    /// several for a multi-episode release); for movies, the movie nodes.
    async fn candidates_for_title(
        &self,
        media_type: MediaType,
        title_query: &str,
    ) -> Result<Vec<ContentCandidate>, Self::Error>;
}
