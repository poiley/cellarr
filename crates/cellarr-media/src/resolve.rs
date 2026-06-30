//! The content-metadata resolver seam.
//!
//! Identify and RefreshMetadata need to attach the resolved facts (year,
//! overview, runtime, air/release dates) and the artwork for a content node so
//! the detail endpoints and the calendar can read them back. The modules' own
//! [`MetadataLookup`](crate::meta::MetadataLookup) seam carries only the slim
//! search/naming facts (title/aliases/ids); the *rich* facts come from the same
//! metadata provider's full payload.
//!
//! Rather than thread a provider-specific payload through the pipeline, the
//! pipeline depends on this one small seam: given a [`ContentRef`], resolve the
//! [`ContentMetadata`] for that node and any poster/fanart artwork it carries. A
//! real implementation (the daemon wiring) fetches from `cellarr-meta` and caches
//! the artwork bytes; tests supply an in-memory map. Keeping the trait here (not
//! in the wiring crate) lets the runner stay offline-testable with a fake.

use async_trait::async_trait;

use cellarr_core::{ContentMetadata, ContentRef};

/// The kind of cached artwork. Mirrors Sonarr/Radarr's `MediaCover` poster/fanart
/// split; these are the two the normalized metadata schema carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtworkKind {
    /// The portrait poster.
    Poster,
    /// The wide background art.
    Fanart,
}

impl ArtworkKind {
    /// The stable slug used in the cache filename and the `MediaCover` route path
    /// segment (`poster` / `fanart`).
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            ArtworkKind::Poster => "poster",
            ArtworkKind::Fanart => "fanart",
        }
    }
}

/// The resolved, content-scoped metadata for a node: the persisted facts plus the
/// artwork the resolver was able to cache for it.
// No `Eq`: `meta: ContentMetadata` carries an `f32` rating (PartialEq only).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedMetadata {
    /// The facts to persist on the node (year/overview/runtime/dates/title).
    pub meta: ContentMetadata,
    /// The artwork kinds the resolver cached for this node (so the persistence
    /// layer / API knows which `MediaCover` images exist). The bytes themselves
    /// are written to the artwork cache by the resolver; this only records *which*
    /// kinds are available.
    pub artwork: Vec<ArtworkKind>,
}

/// The seam the Identify/Refresh path resolves a node's rich metadata through.
///
/// `resolve` returns `Ok(None)` when the node cannot be identified to a source
/// record (no provider configured, or no match) — that is a normal "nothing to
/// persist" outcome, not an error. An `Err` is reserved for an actual provider
/// failure the caller logs and moves past (a metadata refresh failure never
/// blocks acquisition).
#[async_trait]
pub trait MetadataResolver: Send + Sync {
    /// The typed error this resolver reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Resolve (and cache artwork for) the content node, or `None` when nothing
    /// resolved.
    async fn resolve(&self, content: &ContentRef) -> Result<Option<ResolvedMetadata>, Self::Error>;
}

/// A boxed, type-erased resolver error (the dyn facade's uniform error).
pub type ResolveBoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Object-safe facade over [`MetadataResolver`] with a uniform boxed error, so
/// the runner can hold a resolver behind `dyn` regardless of its concrete error
/// type (the same pattern as [`DynMediaModule`](crate::registry::DynMediaModule)).
#[async_trait]
pub trait DynMetadataResolver: Send + Sync {
    /// See [`MetadataResolver::resolve`].
    async fn resolve(
        &self,
        content: &ContentRef,
    ) -> Result<Option<ResolvedMetadata>, ResolveBoxError>;
}

#[async_trait]
impl<T> DynMetadataResolver for T
where
    T: MetadataResolver,
{
    async fn resolve(
        &self,
        content: &ContentRef,
    ) -> Result<Option<ResolvedMetadata>, ResolveBoxError> {
        MetadataResolver::resolve(self, content)
            .await
            .map_err(|e| Box::new(e) as ResolveBoxError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_kind_slugs_are_stable() {
        assert_eq!(ArtworkKind::Poster.slug(), "poster");
        assert_eq!(ArtworkKind::Fanart.slug(), "fanart");
    }
}
