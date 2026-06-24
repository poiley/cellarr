//! The structural `content` tree repository.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cellarr_core::repo::{ContentRepository, DeletedContent};
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

    /// Read the persisted content-scoped metadata for a node, or `None` when the
    /// node has never been identified/refreshed.
    ///
    /// This is the inherent twin of the [`ContentRepository::metadata`] trait
    /// method (which delegates here): it lets in-crate and sibling-crate callers
    /// — the registry's metadata seam, the detail endpoints — read the node's
    /// real facts (`title`/`year`/…) without first importing the repository
    /// trait. A `None` means the node carries no `content_meta` row yet (its
    /// year/overview are unknown), and the caller degrades gracefully rather than
    /// fabricating facts.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn metadata(&self, id: ContentId) -> Result<Option<ContentMetadata>> {
        let row = sqlx::query(
            "SELECT title, year, overview, runtime, air_date, digital_date
             FROM content_meta WHERE content_id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_content_metadata).transpose()
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

    /// Delete a content node identified by `id`, but only when its `kind` matches
    /// `expected_kind`. Returns the [`DeletedContent`] receipt, or `None` when the
    /// node does not exist or is the wrong kind (so the caller can 404 the
    /// addressed surface). Deletes the whole subtree (a series → its
    /// season/episode descendants), the orphaned `media_file` rows, the FTS index
    /// rows, and the node's history — all in **one** transaction so a crash can
    /// never leave the library half-deleted.
    ///
    /// `content_file` and `content_meta` rows fall away via `ON DELETE CASCADE`
    /// when the content node is removed; the virtual FTS table and `media_file`
    /// (referenced *by* `content_file`, so not reached by the content cascade) are
    /// cleaned explicitly here.
    async fn delete_subtree(
        &self,
        id: ContentId,
        expected_kind: ContentKind,
    ) -> Result<Option<DeletedContent>> {
        let want_kind = kind_to_str(expected_kind)?;
        let id_str = id.to_string();
        // The receipt is filled inside the write transaction and read back out
        // after it commits (the writer job returns `()` on success).
        let receipt: Arc<Mutex<Option<DeletedContent>>> = Arc::new(Mutex::new(None));
        let receipt_inner = Arc::clone(&receipt);

        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    // Guard the kind first: a wrong-kind / missing node deletes
                    // nothing and leaves an empty receipt → the caller 404s.
                    let row = sqlx::query("SELECT kind FROM content WHERE id = ?1")
                        .bind(&id_str)
                        .fetch_optional(&mut *conn)
                        .await?;
                    let Some(row) = row else { return Ok(()) };
                    let kind: String = row.try_get("kind")?;
                    if kind != want_kind {
                        return Ok(());
                    }

                    // 1. Collect the whole subtree (root + descendants) by walking
                    //    the adjacency list breadth-first. A depth/size bound is
                    //    implicit: every id is visited once (we never revisit), so
                    //    even a malformed cycle terminates.
                    let mut ids: Vec<String> = vec![id_str.clone()];
                    let mut frontier: Vec<String> = vec![id_str.clone()];
                    while let Some(parent) = frontier.pop() {
                        let children = sqlx::query("SELECT id FROM content WHERE parent_id = ?1")
                            .bind(&parent)
                            .fetch_all(&mut *conn)
                            .await?;
                        for c in children {
                            let cid: String = c.try_get("id")?;
                            if !ids.contains(&cid) {
                                ids.push(cid.clone());
                                frontier.push(cid);
                            }
                        }
                    }

                    // 2. The media files linked anywhere under the subtree, and
                    //    their on-disk paths (the receipt the file step recycles).
                    let mut media_ids: Vec<String> = Vec::new();
                    let mut media_paths: Vec<String> = Vec::new();
                    for cid in &ids {
                        let rows = sqlx::query(
                            "SELECT mf.id AS id, mf.path AS path
                             FROM content_file cf
                             JOIN media_file mf ON mf.id = cf.media_file_id
                             WHERE cf.content_id = ?1",
                        )
                        .bind(cid)
                        .fetch_all(&mut *conn)
                        .await?;
                        for r in rows {
                            let mid: String = r.try_get("id")?;
                            if !media_ids.contains(&mid) {
                                media_ids.push(mid);
                                media_paths.push(r.try_get("path")?);
                            }
                        }
                    }

                    // 3. Clean the rows the content cascade does NOT reach: the FTS
                    //    virtual table, the per-node history, and any grab whose
                    //    JSON content_ref targets a removed node.
                    for cid in &ids {
                        sqlx::query("DELETE FROM content_fts WHERE content_id = ?1")
                            .bind(cid)
                            .execute(&mut *conn)
                            .await?;
                        sqlx::query("DELETE FROM history WHERE content_id = ?1")
                            .bind(cid)
                            .execute(&mut *conn)
                            .await?;
                        sqlx::query(
                            "DELETE FROM grab WHERE json_extract(content_ref, '$.id') = ?1",
                        )
                        .bind(cid)
                        .execute(&mut *conn)
                        .await?;
                    }

                    // 4. Remove the root node. `parent_id ... ON DELETE CASCADE`
                    //    takes the descendants, `content_file`, and `content_meta`
                    //    with it.
                    sqlx::query("DELETE FROM content WHERE id = ?1")
                        .bind(&id_str)
                        .execute(&mut *conn)
                        .await?;

                    // 5. The media_file rows are referenced *by* content_file (the
                    //    cascade runs the other way), so removing the content does
                    //    not touch them. Delete the now-orphaned files explicitly.
                    for mid in &media_ids {
                        sqlx::query("DELETE FROM media_file WHERE id = ?1")
                            .bind(mid)
                            .execute(&mut *conn)
                            .await?;
                    }

                    let content_ids = ids
                        .iter()
                        .map(|s| parse_uuid("content_id", s).map(ContentId::from_uuid))
                        .collect::<Result<Vec<_>>>()?;
                    *receipt_inner.lock().expect("receipt mutex poisoned") = Some(DeletedContent {
                        content_ids,
                        media_file_paths: media_paths,
                    });
                    Ok(())
                })
            })
            .await?;

        let out = receipt.lock().expect("receipt mutex poisoned").take();
        Ok(out)
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
        ContentRepo::metadata(self, id).await
    }

    async fn delete_movie(&self, id: ContentId) -> Result<Option<DeletedContent>> {
        self.delete_subtree(id, ContentKind::Movie).await
    }

    async fn delete_series(&self, id: ContentId) -> Result<Option<DeletedContent>> {
        self.delete_subtree(id, ContentKind::Series).await
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
