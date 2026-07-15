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
            // A top-level container root (Series/Artist/Author) is never an import
            // or grab target — it carries only PLACEHOLDER coordinates (a series is
            // Episode{s1,e1}), which would otherwise collide with the real S01E01 /
            // disc-1-track-1 leaf and let a file adopt onto the container instead of
            // its episode/track. Only leaf and season/album nodes are matchable.
            if matches!(
                node.kind,
                cellarr_core::ContentKind::Series
                    | cellarr_core::ContentKind::Artist
                    | cellarr_core::ContentKind::Author
            ) {
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
/// `title_for` query) and the persisted content-scoped metadata row
/// (`content_meta`). The title — used to build search terms and to render naming
/// tokens — comes from the title cellarr already indexes on ingest, so the
/// pipeline's Discover/Identify/Rename stages have a real title to work with
/// rather than reporting every node unresolved. The **release year** (and, when
/// present, the persisted title) is read from the `content_meta` row written at
/// Identify/Refresh, so an identified movie's `{Release Year}` token renders and
/// its commit lands under `Title (Year)/…`.
///
/// A node with no `content_meta` row simply reports `year: None` (the naming
/// engine then drops the optional `{Release Year}` token gracefully); a node with
/// no indexed title at all still reports unresolved (graceful degrade) rather
/// than fabricating an identity.
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
        let Some(title) = self.resolve_title(content).await? else {
            return Ok(None);
        };
        let year = self.persisted_year(content).await?;
        Ok(Some(MovieMeta {
            title,
            aliases: Vec::new(),
            year,
            external_ids: Vec::new(),
        }))
    }

    async fn series_meta(
        &self,
        content: ContentId,
        _title_id: Option<TitleId>,
    ) -> Result<Option<SeriesMeta>, Self::Error> {
        let Some(title) = self.resolve_title(content).await? else {
            return Ok(None);
        };
        let year = self.persisted_year(content).await?;
        Ok(Some(SeriesMeta {
            title,
            aliases: Vec::new(),
            year,
            external_ids: Vec::new(),
        }))
    }
}

impl DbMetadataLookup {
    /// Resolve a node's identity title: the persisted `content_meta.title` when an
    /// Identify/Refresh has written one, else the FTS-indexed title from ingest.
    /// `None` when neither exists — the node has no usable identity yet, so the
    /// module reports it unresolved rather than fabricating a title.
    async fn resolve_title(&self, content: ContentId) -> Result<Option<String>, DbError> {
        let repo = self.db.content();
        if let Some(meta) = repo.metadata(content).await? {
            if let Some(title) = meta.title.filter(|t| !t.trim().is_empty()) {
                return Ok(Some(title));
            }
        }
        repo.title_for(content).await
    }

    /// The persisted release year for a node, read from the `content_meta` row.
    /// `None` when the node has no metadata row (or the row carries no year) — the
    /// naming engine then drops the optional `{Release Year}` token gracefully.
    async fn persisted_year(&self, content: ContentId) -> Result<Option<u16>, DbError> {
        Ok(self
            .db
            .content()
            .metadata(content)
            .await?
            .and_then(|m| m.year))
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

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::repo::ContentRepository;
    use cellarr_core::{ContentKind, ContentNode, Coordinates, LibraryId, SeriesType};
    use cellarr_media::ContentLookup;

    // A file's title FTS-hits both the series container (placeholder coords S01E01)
    // and the real S01E01 episode. The container must NOT be offered as a candidate,
    // else an S01E01 file adopts onto the series instead of its episode.
    #[tokio::test]
    async fn candidates_for_title_excludes_the_series_container() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
            .await
            .unwrap();
        let library_id = LibraryId::new();
        db.config()
            .upsert_library(&cellarr_core::Library {
                id: library_id,
                media_type: MediaType::Tv,
                name: "tv".into(),
                root_folders: vec!["/tv".into()],
                default_quality_profile: cellarr_core::QualityProfileId::new(),
            })
            .await
            .unwrap();
        let mk = |kind, parent, coords| ContentNode {
            id: ContentId::new(),
            library_id,
            media_type: MediaType::Tv,
            parent_id: parent,
            kind,
            series_type: SeriesType::Standard,
            coords,
            monitored: true,
            title_id: None,
            tags: Vec::new(),
        };
        let series = mk(
            ContentKind::Series,
            None,
            Coordinates::Episode { season: 1, episode: 1, absolute: None },
        );
        let season = mk(
            ContentKind::Season,
            Some(series.id),
            Coordinates::Episode { season: 1, episode: 0, absolute: None },
        );
        let episode = mk(
            ContentKind::Episode,
            Some(season.id),
            Coordinates::Episode { season: 1, episode: 1, absolute: None },
        );
        for n in [&series, &season, &episode] {
            db.content().upsert(n).await.unwrap();
            db.content().index_title(n.id, "Test Show").await.unwrap();
        }

        let lookup = DbContentLookup::new(db);
        let ids: Vec<ContentId> = lookup
            .candidates_for_title(MediaType::Tv, "Test Show")
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.content_ref.id)
            .collect();
        assert!(
            ids.contains(&episode.id),
            "the real S01E01 episode is a candidate"
        );
        assert!(
            !ids.contains(&series.id),
            "the series container must be excluded (placeholder coords collide with S01E01)"
        );
    }
}
