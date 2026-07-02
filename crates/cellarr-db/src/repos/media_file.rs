//! The `media_file` repository.
//!
//! A media file is its own aggregate (a single file can satisfy several content
//! nodes — a multi-episode `.mkv`), so it has a dedicated repository rather than
//! hanging off [`crate::repos::ContentRepo`]. The many-to-many link to content
//! lives in the `content_file` table; [`MediaFileRepo::link`] writes that edge and
//! [`MediaFileRepository::list_for_content`] resolves through it.

use async_trait::async_trait;
use cellarr_core::profile::Quality;
use cellarr_core::repo::MediaFileRepository;
use cellarr_core::{ContentId, MediaFile, MediaFileId};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::convert::parse_uuid;
use crate::error::{DbError, Result};
use crate::writer::WriterHandle;

/// Reads/writes for `media_file` rows.
#[derive(Clone)]
pub struct MediaFileRepo {
    pool: SqlitePool,
    writer: WriterHandle,
}

impl MediaFileRepo {
    pub(crate) fn new(pool: SqlitePool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }

    /// Link a media file to a content node (one edge of the many-to-many
    /// relationship). Idempotent: re-linking the same pair is a no-op.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn link(&self, content: ContentId, file: MediaFileId) -> Result<()> {
        let content = content.to_string();
        let file = file.to_string();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO content_file (content_id, media_file_id)
                         VALUES (?1, ?2)
                         ON CONFLICT(content_id, media_file_id) DO NOTHING",
                    )
                    .bind(content)
                    .bind(file)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// The content nodes a media file is linked to (the reverse of
    /// [`MediaFileRepository::list_for_content`]). A single file can satisfy
    /// several nodes (a multi-episode `.mkv`); the v3 `episodefile`/`moviefile`
    /// resources use this to resolve a file's owning content (and thus its library
    /// root boundary for a crash-safe recycle, and its `seriesId`/`movieId` for
    /// filtering). Returns an empty vec for an unlinked or missing file.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn content_ids_for_file(&self, file: MediaFileId) -> Result<Vec<ContentId>> {
        let rows = sqlx::query("SELECT content_id FROM content_file WHERE media_file_id = ?1")
            .bind(file.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                let id: String = row.try_get("content_id")?;
                Ok(ContentId::from_uuid(parse_uuid("content_id", &id)?))
            })
            .collect()
    }
}

fn row_to_media_file(row: sqlx::sqlite::SqliteRow) -> Result<MediaFile> {
    let id: String = row.try_get("id")?;
    let path: String = row.try_get("path")?;
    let size: i64 = row.try_get("size")?;
    let languages: String = row.try_get("languages")?;
    let quality: String = row.try_get("quality")?;
    let media_info: Option<String> = row.try_get("media_info")?;
    let custom_format_score: Option<i64> = row.try_get("custom_format_score")?;
    let release_type: Option<String> = row.try_get("release_type")?;

    let languages: Vec<String> = serde_json::from_str(&languages)?;
    let quality: Quality = serde_json::from_str(&quality)?;
    let media_info = media_info
        .map(|m| serde_json::from_str(&m).map_err(DbError::from))
        .transpose()?;
    let release_type = crate::repos::grab::release_type_from_str(release_type)?;

    Ok(MediaFile {
        id: MediaFileId::from_uuid(parse_uuid("id", &id)?),
        path,
        // Sizes are non-negative; stored as INTEGER (i64) and widened back.
        size: size as u64,
        quality,
        languages,
        media_info,
        custom_format_score: custom_format_score.map(|s| s as i32),
        release_type,
    })
}

#[async_trait]
impl MediaFileRepository for MediaFileRepo {
    type Error = DbError;

    async fn create(&self, file: &MediaFile) -> Result<()> {
        let id = file.id.to_string();
        let path = file.path.clone();
        // u64 -> i64 for SQLite's signed INTEGER; library file sizes never
        // approach i64::MAX, so the cast is lossless in practice.
        let size = file.size as i64;
        let languages = serde_json::to_string(&file.languages)?;
        let quality = serde_json::to_string(&file.quality)?;
        let quality_rank = i64::from(file.quality.rank);
        let media_info = file
            .media_info
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let custom_format_score = file.custom_format_score.map(i64::from);
        let release_type = crate::repos::grab::release_type_to_str(file.release_type)?;

        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO media_file
                            (id, path, size, languages, quality, quality_rank,
                             media_info, custom_format_score, release_type)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    )
                    .bind(id)
                    .bind(path)
                    .bind(size)
                    .bind(languages)
                    .bind(quality)
                    .bind(quality_rank)
                    .bind(media_info)
                    .bind(custom_format_score)
                    .bind(release_type)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    async fn get(&self, id: MediaFileId) -> Result<Option<MediaFile>> {
        let row = sqlx::query(
            "SELECT id, path, size, languages, quality, media_info, custom_format_score,
                    release_type
             FROM media_file WHERE id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_media_file).transpose()
    }

    async fn find_by_path(&self, path: &str) -> Result<Option<MediaFile>> {
        let row = sqlx::query(
            "SELECT id, path, size, languages, quality, media_info, custom_format_score,
                    release_type
             FROM media_file WHERE path = ?1",
        )
        .bind(path)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_media_file).transpose()
    }

    async fn all_paths(&self) -> Result<Vec<String>> {
        let rows = sqlx::query("SELECT path FROM media_file")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| row.try_get("path").map_err(DbError::from))
            .collect()
    }

    async fn list_for_content(&self, content: ContentId) -> Result<Vec<MediaFile>> {
        let rows = sqlx::query(
            "SELECT m.id, m.path, m.size, m.languages, m.quality, m.media_info,
                    m.custom_format_score, m.release_type
             FROM media_file m
             JOIN content_file cf ON cf.media_file_id = m.id
             WHERE cf.content_id = ?1
             ORDER BY m.path ASC",
        )
        .bind(content.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_media_file).collect()
    }

    async fn delete(&self, id: MediaFileId) -> Result<()> {
        let id = id.to_string();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    // ON DELETE CASCADE on content_file clears any links.
                    sqlx::query("DELETE FROM media_file WHERE id = ?1")
                        .bind(id)
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .await
    }

    async fn delete_by_path(&self, path: &str) -> Result<()> {
        let path = path.to_string();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    // ON DELETE CASCADE on content_file clears any links.
                    sqlx::query("DELETE FROM media_file WHERE path = ?1")
                        .bind(path)
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .await
    }
}
