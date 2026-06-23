//! The `grab` repository.

use async_trait::async_trait;
use cellarr_core::repo::GrabRepository;
use cellarr_core::{ContentRef, GrabId, GrabRequest, Release};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;
use time::OffsetDateTime;

use crate::convert::format_time;
use crate::error::{DbError, Result};
use crate::writer::WriterHandle;

/// Lifecycle state of a grab. Persisted as a lowercase string in `grab.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrabStatus {
    /// Sent to the client, not yet acknowledged complete.
    Queued,
    /// Download finished, ready to import.
    Completed,
    /// Download failed.
    Failed,
    /// Files imported into the library.
    Imported,
}

impl GrabStatus {
    fn as_str(self) -> &'static str {
        match self {
            GrabStatus::Queued => "queued",
            GrabStatus::Completed => "completed",
            GrabStatus::Failed => "failed",
            GrabStatus::Imported => "imported",
        }
    }
}

/// Reads/writes for grabs handed to download clients.
#[derive(Clone)]
pub struct GrabRepo {
    pool: SqlitePool,
    writer: WriterHandle,
}

impl GrabRepo {
    pub(crate) fn new(pool: SqlitePool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }

    /// Record the download client's id and advance status for a grab.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn set_download_id(
        &self,
        id: GrabId,
        download_id: &str,
        status: GrabStatus,
    ) -> Result<()> {
        let id = id.to_string();
        let download_id = download_id.to_string();
        let status = status.as_str();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query("UPDATE grab SET download_id = ?2, status = ?3 WHERE id = ?1")
                        .bind(id)
                        .bind(download_id)
                        .bind(status)
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .await
    }
}

#[async_trait]
impl GrabRepository for GrabRepo {
    type Error = DbError;

    async fn create(&self, request: &GrabRequest) -> Result<GrabId> {
        let id = GrabId::new();
        let id_str = id.to_string();
        let content_ref = serde_json::to_string(&request.content_ref)?;
        let release = serde_json::to_string(&request.release)?;
        let indexer_id = request.indexer_id.to_string();
        let client_id = request.client_id.to_string();
        let category = request.category.clone();
        let created_at = format_time(OffsetDateTime::now_utc())?;

        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO grab
                            (id, content_ref, release, indexer_id, client_id, category,
                             download_id, status, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 'queued', ?7)",
                    )
                    .bind(id_str)
                    .bind(content_ref)
                    .bind(release)
                    .bind(indexer_id)
                    .bind(client_id)
                    .bind(category)
                    .bind(created_at)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await?;
        Ok(id)
    }

    async fn get(&self, id: GrabId) -> Result<Option<GrabRequest>> {
        let row = sqlx::query(
            "SELECT content_ref, release, indexer_id, client_id, category
             FROM grab WHERE id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let content_ref: String = row.try_get("content_ref")?;
        let release: String = row.try_get("release")?;
        let indexer_id: String = row.try_get("indexer_id")?;
        let client_id: String = row.try_get("client_id")?;
        let category: String = row.try_get("category")?;

        let content_ref: ContentRef = serde_json::from_str(&content_ref)?;
        let release: Release = serde_json::from_str(&release)?;
        let indexer_id =
            serde_json::from_value(serde_json::Value::String(indexer_id)).map_err(DbError::from)?;
        let client_id =
            serde_json::from_value(serde_json::Value::String(client_id)).map_err(DbError::from)?;

        Ok(Some(GrabRequest {
            content_ref,
            release,
            indexer_id,
            client_id,
            category,
        }))
    }
}
