//! The structural `content` tree repository.

use async_trait::async_trait;
use cellarr_core::repo::ContentRepository;
use cellarr_core::{ContentId, ContentRef, Coordinates, LibraryId, MediaType, TitleId};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::convert::parse_uuid;
use crate::error::{DbError, Result};
use crate::writer::WriterHandle;

/// A full structural content node, as persisted. The pipeline only carries the
/// lighter [`ContentRef`]; this richer struct is for ingest/management.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentNode {
    /// Node id.
    pub id: ContentId,
    /// Owning library.
    pub library_id: LibraryId,
    /// Media type (carried so callers pick the right `MediaModule`).
    pub media_type: MediaType,
    /// Parent node in the adjacency list, if any.
    pub parent_id: Option<ContentId>,
    /// Structural kind (series/season/episode/movie/...).
    pub kind: String,
    /// Numbering within the type.
    pub coords: Coordinates,
    /// Whether the node is monitored for acquisition.
    pub monitored: bool,
    /// Link to the typed identity row, if resolved.
    pub title_id: Option<TitleId>,
}

impl ContentNode {
    /// The lighter handle the pipeline carries.
    #[must_use]
    pub fn as_ref(&self) -> ContentRef {
        ContentRef {
            id: self.id,
            library_id: self.library_id,
            media_type: self.media_type,
            coords: self.coords.clone(),
        }
    }
}

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

    /// Insert or replace a content node.
    ///
    /// # Errors
    /// Returns a [`DbError`] on serialization or write failure.
    pub async fn upsert(&self, node: &ContentNode) -> Result<()> {
        let id = node.id.to_string();
        let library_id = node.library_id.to_string();
        let media_type = serde_json::to_value(node.media_type)?
            .as_str()
            .unwrap_or_default()
            .to_string();
        let parent_id = node.parent_id.map(|p| p.to_string());
        let kind = node.kind.clone();
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

    /// Fetch a full node (not just the [`ContentRef`]).
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
}
