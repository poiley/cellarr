//! Building the runtime registries at boot.
//!
//! The daemon holds a [`MediaRegistry`] of the media-type modules it serves; the
//! pipeline asks it for the module matching a content node and never names a
//! concrete type (`docs/01-architecture.md`). This is the one place those modules
//! are constructed and their seams bound to concrete `cellarr-db` repositories.
//!
//! The modules read through two seams (`cellarr-media`): a [`ContentLookup`] (find
//! candidate nodes by title) and a [`MetadataLookup`] (the linked movie/series
//! identity). The content seam is backed here by the DB's FTS title search — fully
//! functional. The metadata seam is **not yet** backed by a public identity-link
//! query on `cellarr-db`; until that lands it reports identity as unresolved
//! rather than reaching into another crate's SQL. This is a documented core gap:
//! the registry enumerates the supported media types and matching works; search
//! term/naming generation waits on the identity-link repository API.

use async_trait::async_trait;
use cellarr_core::{ContentId, MediaType, TitleId};
use cellarr_db::{Database, DbError};
use cellarr_media::{
    ContentCandidate, ContentLookup, MediaRegistry, MetadataLookup, MovieMeta, MovieModule,
    SeriesMeta, TvModule,
};

/// Build the media registry the daemon serves.
///
/// Registers a module per supported media type (Movie, Tv in v1), each wired to
/// the database. Adding a media type here is the whole change — the pipeline does
/// not branch on [`MediaType`].
#[must_use]
pub fn build_media_registry(db: &Database) -> MediaRegistry {
    let mut registry = MediaRegistry::new();
    registry.register(MovieModule::new(
        DbContentLookup::new(db.clone()),
        DbMetadataLookup::new(db.clone()),
    ));
    registry.register(TvModule::new(
        DbContentLookup::new(db.clone()),
        DbMetadataLookup::new(db.clone()),
    ));
    registry
}

/// A [`ContentLookup`] backed by the DB's FTS title index.
struct DbContentLookup {
    db: Database,
}

impl DbContentLookup {
    fn new(db: Database) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ContentLookup for DbContentLookup {
    type Error = DbError;

    async fn candidates_for_title(
        &self,
        media_type: MediaType,
        title_query: &str,
    ) -> Result<Vec<ContentCandidate>, Self::Error> {
        let content = self.db.content();
        let ids = content.search(title_query).await?;
        let mut out = Vec::new();
        for id in ids {
            // Resolve each hit to its node; a node that vanished between the FTS
            // hit and this read is simply skipped rather than failing the query.
            let Some(node) = content.get_node(id).await? else {
                continue;
            };
            if node.media_type != media_type {
                continue;
            }
            // The candidate's title is the indexed title the FTS hit matched on,
            // read back so `match_release`'s title-confidence check compares the
            // parsed release title against a real title rather than a node id.
            let title = content
                .title_for(id)
                .await?
                .unwrap_or_else(|| node_title(&node));
            out.push(ContentCandidate {
                content_ref: node.as_ref(),
                title,
                aliases: Vec::new(),
            });
        }
        Ok(out)
    }
}

/// A [`MetadataLookup`] backed by the DB's indexed content title (the FTS
/// `title_for` query). This resolves a node's identity — the title used to build
/// search terms and to render naming tokens — directly from the title cellarr
/// already indexes on ingest, so the pipeline's Discover/Identify/Rename stages
/// have a real title to work with rather than reporting every node unresolved.
/// External ids / year are not yet modeled on the content row, so they are left
/// empty; a node with no indexed title still reports unresolved (graceful
/// degrade) rather than fabricating one.
struct DbMetadataLookup {
    db: Database,
}

impl DbMetadataLookup {
    fn new(db: Database) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MetadataLookup for DbMetadataLookup {
    type Error = DbError;

    async fn movie_meta(
        &self,
        content: ContentId,
        _title_id: Option<TitleId>,
    ) -> Result<Option<MovieMeta>, Self::Error> {
        let Some(title) = self.db.content().title_for(content).await? else {
            return Ok(None);
        };
        Ok(Some(MovieMeta {
            title,
            aliases: Vec::new(),
            year: None,
            external_ids: Vec::new(),
        }))
    }

    async fn series_meta(
        &self,
        content: ContentId,
        _title_id: Option<TitleId>,
    ) -> Result<Option<SeriesMeta>, Self::Error> {
        let Some(title) = self.db.content().title_for(content).await? else {
            return Ok(None);
        };
        Ok(Some(SeriesMeta {
            title,
            aliases: Vec::new(),
            year: None,
            external_ids: Vec::new(),
        }))
    }
}

/// A node's display/search title. The content row carries no title column itself
/// (titles live in the FTS index and the identity tables), so derive a stable
/// label from what the node row does carry. Used only for the candidate's
/// `title`, which `match_release` re-checks against the parse.
fn node_title(node: &cellarr_core::ContentNode) -> String {
    node.title_id
        .map(|t| t.to_string())
        .unwrap_or_else(|| node.id.to_string())
}
