//! The structural `content` tree repository.

use async_trait::async_trait;
use cellarr_core::repo::ContentRepository;
use cellarr_core::{
    ContentId, ContentKind, ContentMetadata, ContentNode, ContentRef, Coordinates, LibraryId,
    MediaType, TitleId,
};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::convert::parse_uuid;
use crate::error::{DbError, Result};
use crate::writer::WriterHandle;

/// Reads/writes for the `content` adjacency list.
#[derive(Clone)]
pub struct ContentRepo {
    pool: SqlitePool,
    writer: WriterHandle,
}

impl ContentRepo {
    pub(crate) fn new(pool: SqlitePool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }

    /// Index (or re-index) a node's searchable title in the FTS table.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn index_title(&self, id: ContentId, title: &str) -> Result<()> {
        let id = id.to_string();
        let title = title.to_string();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query("DELETE FROM content_fts WHERE content_id = ?1")
                        .bind(&id)
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("INSERT INTO content_fts (content_id, title) VALUES (?1, ?2)")
                        .bind(&id)
                        .bind(&title)
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Full-text search content titles, returning matching node ids best-first.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query failure.
    pub async fn search(&self, query: &str) -> Result<Vec<ContentId>> {
        let rows = sqlx::query(
            "SELECT content_id FROM content_fts WHERE content_fts MATCH ?1 ORDER BY rank",
        )
        .bind(query)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                let s: String = r.try_get("content_id")?;
                Ok(ContentId::from_uuid(parse_uuid("content_id", &s)?))
            })
            .collect()
    }

    /// Recover the searchable title indexed for a node, if one was indexed.
    ///
    /// The `content` row carries no title column (titles live in the FTS index),
    /// so this is the reverse of [`index_title`](Self::index_title): it lets the
    /// list resources surface a node's real title instead of its UUID. `None`
    /// means the node has no indexed title (it was never identified/added with a
    /// title), and the caller falls back to the id.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query failure.
    pub async fn title_for(&self, id: ContentId) -> Result<Option<String>> {
        let row = sqlx::query("SELECT title FROM content_fts WHERE content_id = ?1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| r.try_get::<String, _>("title").map_err(DbError::from))
            .transpose()
    }

    /// Resolve a content node to the **TVDB id of the series it belongs to**.
    ///
    /// This is the identity-link query the anime absolute→episode remap is gated
    /// on: Identify needs the series' external id to select the right scene
    /// mapping. It walks the structural tree up from `id` to the series root
    /// (following `parent_id`), reads that node's `title_id`, and looks up
    /// `series_meta.tvdb_id` for it.
    ///
    /// Returns `None` when the node has no series ancestor, the series is not yet
    /// identity-linked (`title_id` is null), or the linked `series_meta` carries
    /// no `tvdb_id`. A `None` here means "identity unresolved" — the caller
    /// surfaces the absolute number for manual resolution rather than guessing
    /// (the library-safety rule), never an error.
    ///
    /// The walk is bounded by a depth cap so a malformed cycle in the adjacency
    /// list can never spin forever (the TV tree is at most series→season→episode,
    /// so a handful of hops suffices).
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn series_tvdb_id(&self, id: ContentId) -> Result<Option<i64>> {
        // Walk to the root of this node's tree. The series node is the root of a
        // TV content tree (series→season→episode); a depth cap guards against a
        // malformed parent cycle.
        const MAX_DEPTH: usize = 8;
        let mut current = id;
        let mut title_id: Option<String> = None;
        for _ in 0..MAX_DEPTH {
            let row = sqlx::query("SELECT parent_id, title_id FROM content WHERE id = ?1")
                .bind(current.to_string())
                .fetch_optional(&self.pool)
                .await?;
            let Some(row) = row else {
                // The node (or a parent link) does not exist; unresolved.
                return Ok(None);
            };
            let parent_id: Option<String> = row.try_get("parent_id")?;
            let node_title_id: Option<String> = row.try_get("title_id")?;
            match parent_id {
                Some(parent) => {
                    // Not the root yet; keep the deepest title_id we have seen as a
                    // fallback but prefer the root series node's link.
                    current = ContentId::from_uuid(parse_uuid("parent_id", &parent)?);
                }
                None => {
                    // Reached the series root; its title_id is the series identity.
                    title_id = node_title_id;
                    break;
                }
            }
        }

        let Some(title_id) = title_id else {
            return Ok(None);
        };

        let row = sqlx::query("SELECT tvdb_id FROM series_meta WHERE title_id = ?1")
            .bind(title_id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => Ok(row.try_get::<Option<i64>, _>("tvdb_id")?),
            None => Ok(None),
        }
    }

    /// Fetch a full [`ContentNode`] (not just the [`ContentRef`]).
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn get_node(&self, id: ContentId) -> Result<Option<ContentNode>> {
        let row = sqlx::query(
            "SELECT id, library_id, media_type, parent_id, kind, coords, monitored, title_id
             FROM content WHERE id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_node).transpose()
    }
}

/// Serialize a [`ContentKind`] to its stored lowercase string form.
///
/// `ContentKind` serializes to a bare JSON string (`"episode"`); we want the raw
/// scalar for the `content.kind` TEXT column, so unwrap the JSON string. The
/// `unwrap_or_default` can never actually fire (the enum always serializes to a
/// string), but avoids a panic on the fallible runtime path.
fn kind_to_str(kind: ContentKind) -> Result<String> {
    Ok(serde_json::to_value(kind)?
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// Parse a stored `content.kind` string back into a [`ContentKind`].
fn kind_from_str(kind: &str) -> Result<ContentKind> {
    serde_json::from_value(serde_json::Value::String(kind.to_string())).map_err(DbError::from)
}

fn row_to_node(row: sqlx::sqlite::SqliteRow) -> Result<ContentNode> {
    let id: String = row.try_get("id")?;
    let library_id: String = row.try_get("library_id")?;
    let media_type: String = row.try_get("media_type")?;
    let parent_id: Option<String> = row.try_get("parent_id")?;
    let kind: String = row.try_get("kind")?;
    let coords: String = row.try_get("coords")?;
    let monitored: i64 = row.try_get("monitored")?;
    let title_id: Option<String> = row.try_get("title_id")?;

    let media_type: MediaType =
        serde_json::from_value(serde_json::Value::String(media_type)).map_err(DbError::from)?;
    let kind = kind_from_str(&kind)?;
    let coords: Coordinates = serde_json::from_str(&coords)?;
    let parent_id = parent_id
        .map(|p| parse_uuid("parent_id", &p).map(ContentId::from_uuid))
        .transpose()?;
    let title_id = title_id
        .map(|t| parse_uuid("title_id", &t).map(TitleId::from_uuid))
        .transpose()?;

    Ok(ContentNode {
        id: ContentId::from_uuid(parse_uuid("id", &id)?),
        library_id: LibraryId::from_uuid(parse_uuid("library_id", &library_id)?),
        media_type,
        parent_id,
        kind,
        coords,
        monitored: monitored != 0,
        title_id,
    })
}

#[async_trait]
impl ContentRepository for ContentRepo {
    type Error = DbError;

    async fn get(&self, id: ContentId) -> Result<Option<ContentRef>> {
        Ok(self.get_node(id).await?.map(|n| n.as_ref()))
    }

    async fn monitored_missing(&self) -> Result<Vec<ContentRef>> {
        // Monitored nodes with no linked media_file are "missing". Containers
        // (series/season/artist/album/author) are excluded: only leaf, grabbable
        // nodes are acquisition targets.
        let rows = sqlx::query(
            "SELECT c.id, c.library_id, c.media_type, c.parent_id, c.kind, c.coords,
                    c.monitored, c.title_id
             FROM content c
             WHERE c.monitored = 1
               AND c.kind IN ('movie', 'episode', 'track', 'book')
               AND NOT EXISTS (
                   SELECT 1 FROM content_file cf WHERE cf.content_id = c.id
               )",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| row_to_node(r).map(|n| n.as_ref()))
            .collect()
    }

    async fn upsert(&self, node: &ContentNode) -> Result<()> {
        let id = node.id.to_string();
        let library_id = node.library_id.to_string();
        let media_type = serde_json::to_value(node.media_type)?
            .as_str()
            .unwrap_or_default()
            .to_string();
        let parent_id = node.parent_id.map(|p| p.to_string());
        let kind = kind_to_str(node.kind)?;
        let coords = serde_json::to_string(&node.coords)?;
        let monitored = i64::from(node.monitored);
        let title_id = node.title_id.map(|t| t.to_string());

        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO content
                            (id, library_id, media_type, parent_id, kind, coords, monitored, title_id)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                         ON CONFLICT(id) DO UPDATE SET
                            library_id = excluded.library_id,
                            media_type = excluded.media_type,
                            parent_id  = excluded.parent_id,
                            kind       = excluded.kind,
                            coords     = excluded.coords,
                            monitored  = excluded.monitored,
                            title_id   = excluded.title_id",
                    )
                    .bind(id)
                    .bind(library_id)
                    .bind(media_type)
                    .bind(parent_id)
                    .bind(kind)
                    .bind(coords)
                    .bind(monitored)
                    .bind(title_id)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    async fn children(&self, parent: ContentId) -> Result<Vec<ContentNode>> {
        // Ordered by id for a stable, deterministic walk; coords ordering would
        // require parsing the tagged JSON, which the adjacency-list walk does not
        // need. Callers that want numbering order sort on the decoded coords.
        let rows = sqlx::query(
            "SELECT id, library_id, media_type, parent_id, kind, coords, monitored, title_id
             FROM content WHERE parent_id = ?1 ORDER BY id ASC",
        )
        .bind(parent.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_node).collect()
    }

    async fn roots(&self, library: LibraryId) -> Result<Vec<ContentNode>> {
        // Root nodes have no parent: a flat movie, or a series/artist/author the
        // tree hangs off of. Ordered by id for a stable list.
        let rows = sqlx::query(
            "SELECT id, library_id, media_type, parent_id, kind, coords, monitored, title_id
             FROM content WHERE library_id = ?1 AND parent_id IS NULL ORDER BY id ASC",
        )
        .bind(library.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_node).collect()
    }

    async fn set_metadata(&self, id: ContentId, meta: &ContentMetadata) -> Result<()> {
        let id = id.to_string();
        let title = meta.title.clone();
        let year = meta.year.map(i64::from);
        let overview = meta.overview.clone();
        let runtime = meta.runtime.map(i64::from);
        let air_date = meta.air_date.clone();
        let digital_date = meta.digital_date.clone();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO content_meta
                            (content_id, title, year, overview, runtime, air_date, digital_date)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         ON CONFLICT(content_id) DO UPDATE SET
                            title        = excluded.title,
                            year         = excluded.year,
                            overview     = excluded.overview,
                            runtime      = excluded.runtime,
                            air_date     = excluded.air_date,
                            digital_date = excluded.digital_date",
                    )
                    .bind(id)
                    .bind(title)
                    .bind(year)
                    .bind(overview)
                    .bind(runtime)
                    .bind(air_date)
                    .bind(digital_date)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    async fn metadata(&self, id: ContentId) -> Result<Option<ContentMetadata>> {
        let row = sqlx::query(
            "SELECT title, year, overview, runtime, air_date, digital_date
             FROM content_meta WHERE content_id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_content_metadata).transpose()
    }
}

/// Decode a `content_meta` row into a [`ContentMetadata`]. The integer columns are
/// stored as SQLite `INTEGER` (i64) and narrowed back to the domain widths; a
/// value out of range is treated as absent rather than panicking (the metadata
/// source never emits a negative year/runtime, but the read path must stay
/// total).
fn row_to_content_metadata(row: sqlx::sqlite::SqliteRow) -> Result<ContentMetadata> {
    let year: Option<i64> = row.try_get("year")?;
    let runtime: Option<i64> = row.try_get("runtime")?;
    Ok(ContentMetadata {
        title: row.try_get("title")?,
        year: year.and_then(|y| u16::try_from(y).ok()),
        overview: row.try_get("overview")?,
        runtime: runtime.and_then(|r| u32::try_from(r).ok()),
        air_date: row.try_get("air_date")?,
        digital_date: row.try_get("digital_date")?,
    })
}
