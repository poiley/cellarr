//! The `subtitle` repository.
//!
//! A subtitle sidecar belongs to a single [`cellarr_core::MediaFile`] (the file
//! it accompanies on disk), so it hangs off that aggregate via `media_file_id`
//! with `ON DELETE CASCADE` — deleting a file clears its subtitle rows. One row
//! per `(media_file, language, forced/hearing-impaired variant)`; a re-fetch of
//! the same variant UPSERTs in place.

use sqlx::Row;

use cellarr_core::{ContentId, MediaFileId, Subtitle, SubtitleId};

use crate::convert::parse_uuid;
use crate::dialect::{pq, DbPool, DbRow};
use crate::error::Result;
use crate::writer::WriterHandle;

/// Reads/writes for `subtitle` rows.
#[derive(Clone)]
pub struct SubtitleRepo {
    pool: DbPool,
    writer: WriterHandle,
}

impl SubtitleRepo {
    pub(crate) fn new(pool: DbPool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }

    /// Insert a subtitle, or update the existing row for the same
    /// `(media_file, language, forced, hearing_impaired)` variant in place (a
    /// re-fetch that upgrades the file). The passed `id` is kept only when the row
    /// is newly inserted; on conflict the existing row's id is preserved. The
    /// `added_at` stamp is set to now by the repo (the passed value is ignored),
    /// matching how the other repos stamp their timestamps.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn upsert(&self, sub: &Subtitle) -> Result<()> {
        let id = sub.id.to_string();
        let media_file_id = sub.media_file_id.to_string();
        let language = sub.language.clone();
        let path = sub.path.clone();
        let provider = sub.provider.clone();
        let provider_id = sub.provider_id.clone();
        let score = sub.score.map(i64::from);
        let forced = i64::from(sub.forced);
        let hearing_impaired = i64::from(sub.hearing_impaired);
        let added_at = crate::convert::format_time(time::OffsetDateTime::now_utc())?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq(
                        "INSERT INTO subtitle
                            (id, media_file_id, language, path, provider, provider_id,
                             score, forced, hearing_impaired, added_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                         ON CONFLICT(media_file_id, language, forced, hearing_impaired)
                         DO UPDATE SET
                            path = excluded.path,
                            provider = excluded.provider,
                            provider_id = excluded.provider_id,
                            score = excluded.score,
                            added_at = excluded.added_at",
                    ))
                    .bind(id)
                    .bind(media_file_id)
                    .bind(language)
                    .bind(path)
                    .bind(provider)
                    .bind(provider_id)
                    .bind(score)
                    .bind(forced)
                    .bind(hearing_impaired)
                    .bind(added_at)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Every subtitle recorded for a media file, newest first.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn list_for_file(&self, file: MediaFileId) -> Result<Vec<Subtitle>> {
        let rows = sqlx::query(&pq(
            "SELECT id, media_file_id, language, path, provider, provider_id,
                    score, forced, hearing_impaired, added_at
             FROM subtitle WHERE media_file_id = ?1
             ORDER BY added_at DESC, language ASC",
        ))
        .bind(file.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_subtitle).collect()
    }

    /// Every subtitle for the media files linked to a content node — what a
    /// movie/episode detail view shows as its subtitle coverage.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn list_for_content(&self, content: ContentId) -> Result<Vec<Subtitle>> {
        let rows = sqlx::query(&pq(
            "SELECT s.id, s.media_file_id, s.language, s.path, s.provider, s.provider_id,
                    s.score, s.forced, s.hearing_impaired, s.added_at
             FROM subtitle s
             JOIN content_file cf ON cf.media_file_id = s.media_file_id
             WHERE cf.content_id = ?1
             ORDER BY s.language ASC",
        ))
        .bind(content.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_subtitle).collect()
    }

    /// Delete a subtitle row (the on-disk sidecar is removed separately by the fs
    /// layer). A missing id is a no-op.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn delete(&self, id: SubtitleId) -> Result<()> {
        let id = id.to_string();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq("DELETE FROM subtitle WHERE id = ?1"))
                        .bind(id)
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .await
    }
}

fn row_to_subtitle(row: DbRow) -> Result<Subtitle> {
    let id: String = row.try_get("id")?;
    let media_file_id: String = row.try_get("media_file_id")?;
    let language: String = row.try_get("language")?;
    let path: String = row.try_get("path")?;
    let provider: String = row.try_get("provider")?;
    let provider_id: Option<String> = row.try_get("provider_id")?;
    let score: Option<i64> = row.try_get("score")?;
    let forced: i64 = row.try_get("forced")?;
    let hearing_impaired: i64 = row.try_get("hearing_impaired")?;
    let added_at: String = row.try_get("added_at")?;

    Ok(Subtitle {
        id: SubtitleId::from_uuid(parse_uuid("id", &id)?),
        media_file_id: MediaFileId::from_uuid(parse_uuid("media_file_id", &media_file_id)?),
        language,
        path,
        provider,
        provider_id,
        score: score.map(|s| s as i32),
        forced: forced != 0,
        hearing_impaired: hearing_impaired != 0,
        added_at,
    })
}
