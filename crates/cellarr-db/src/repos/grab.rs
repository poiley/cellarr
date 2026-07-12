//! The `grab` repository.

use async_trait::async_trait;
use cellarr_core::repo::GrabRepository;
use cellarr_core::{ContentRef, Grab, GrabId, GrabRequest, GrabStatus, Release, ReleaseType};
use crate::dialect::{pq, DbPool};
use sqlx::Row;
use time::OffsetDateTime;

use crate::convert::{format_time, parse_time};
use crate::error::{DbError, Result};
use crate::writer::WriterHandle;

/// Serialize a [`GrabStatus`] to its stored lowercase (snake_case) string.
///
/// `GrabStatus` serializes to a bare JSON string; we store that scalar in the
/// `grab.status` TEXT column.
fn status_to_str(status: GrabStatus) -> Result<String> {
    Ok(serde_json::to_value(status)?
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// Parse a stored `grab.status` string back into a [`GrabStatus`].
fn status_from_str(status: &str) -> Result<GrabStatus> {
    serde_json::from_value(serde_json::Value::String(status.to_string())).map_err(DbError::from)
}

/// Serialize an optional [`ReleaseType`] to its stored scalar string, or `None`.
///
/// `ReleaseType` serializes to a bare JSON string (e.g. `"full_season"`); we
/// store that scalar in the nullable `release_type` TEXT column.
pub(crate) fn release_type_to_str(rt: Option<ReleaseType>) -> Result<Option<String>> {
    rt.map(|rt| {
        Ok(serde_json::to_value(rt)?
            .as_str()
            .unwrap_or_default()
            .to_string())
    })
    .transpose()
}

/// Parse a stored `release_type` scalar back into a [`ReleaseType`], when present.
pub(crate) fn release_type_from_str(rt: Option<String>) -> Result<Option<ReleaseType>> {
    rt.map(|rt| serde_json::from_value(serde_json::Value::String(rt)).map_err(DbError::from))
        .transpose()
}

/// Reads/writes for grabs handed to download clients.
#[derive(Clone)]
pub struct GrabRepo {
    pool: DbPool,
    writer: WriterHandle,
}

impl GrabRepo {
    pub(crate) fn new(pool: DbPool, writer: WriterHandle) -> Self {
        Self { pool, writer }
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
        // New grabs start at the core-defined initial state.
        let status = status_to_str(GrabStatus::Pending)?;
        // The durable release type derived from the parse at grab time.
        let release_type = release_type_to_str(request.release_type)?;

        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq(
                        "INSERT INTO grab
                            (id, content_ref, release, indexer_id, client_id, category,
                             download_id, status, created_at, release_type)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8, ?9)"),
                    )
                    .bind(id_str)
                    .bind(content_ref)
                    .bind(release)
                    .bind(indexer_id)
                    .bind(client_id)
                    .bind(category)
                    .bind(status)
                    .bind(created_at)
                    .bind(release_type)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await?;
        Ok(id)
    }

    async fn get(&self, id: GrabId) -> Result<Option<Grab>> {
        let row = sqlx::query(&pq(
            "SELECT content_ref, release, indexer_id, client_id, category, download_id, status,
                    release_type, created_at
             FROM grab WHERE id = ?1"),
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
        let download_id: Option<String> = row.try_get("download_id")?;
        let status: String = row.try_get("status")?;
        let release_type: Option<String> = row.try_get("release_type")?;
        let created_at: String = row.try_get("created_at")?;

        let content_ref: ContentRef = serde_json::from_str(&content_ref)?;
        let release: Release = serde_json::from_str(&release)?;
        let indexer_id =
            serde_json::from_value(serde_json::Value::String(indexer_id)).map_err(DbError::from)?;
        let client_id =
            serde_json::from_value(serde_json::Value::String(client_id)).map_err(DbError::from)?;
        let status = status_from_str(&status)?;
        let release_type = release_type_from_str(release_type)?;
        let created_at = parse_time("created_at", &created_at)?;

        Ok(Some(Grab {
            id,
            request: GrabRequest {
                content_ref,
                release,
                indexer_id,
                client_id,
                category,
                release_type,
            },
            download_id,
            status,
            created_at,
        }))
    }

    async fn list(&self) -> Result<Vec<Grab>> {
        let rows = sqlx::query(&pq(
            "SELECT id, content_ref, release, indexer_id, client_id, category, download_id, status,
                    release_type, created_at
             FROM grab ORDER BY created_at DESC"),
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id_str: String = row.try_get("id")?;
            let content_ref: String = row.try_get("content_ref")?;
            let release: String = row.try_get("release")?;
            let indexer_id: String = row.try_get("indexer_id")?;
            let client_id: String = row.try_get("client_id")?;
            let category: String = row.try_get("category")?;
            let download_id: Option<String> = row.try_get("download_id")?;
            let status: String = row.try_get("status")?;
            let release_type: Option<String> = row.try_get("release_type")?;
            let created_at: String = row.try_get("created_at")?;

            let id =
                serde_json::from_value(serde_json::Value::String(id_str)).map_err(DbError::from)?;
            let content_ref: ContentRef = serde_json::from_str(&content_ref)?;
            let release: Release = serde_json::from_str(&release)?;
            let indexer_id = serde_json::from_value(serde_json::Value::String(indexer_id))
                .map_err(DbError::from)?;
            let client_id = serde_json::from_value(serde_json::Value::String(client_id))
                .map_err(DbError::from)?;
            let status = status_from_str(&status)?;
            let release_type = release_type_from_str(release_type)?;
            let created_at = parse_time("created_at", &created_at)?;

            out.push(Grab {
                id,
                request: GrabRequest {
                    content_ref,
                    release,
                    indexer_id,
                    client_id,
                    category,
                    release_type,
                },
                download_id,
                status,
                created_at,
            });
        }
        Ok(out)
    }

    async fn set_download_id(&self, id: GrabId, download_id: &str) -> Result<()> {
        let id = id.to_string();
        let download_id = download_id.to_string();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq("UPDATE grab SET download_id = ?2 WHERE id = ?1"))
                        .bind(id)
                        .bind(download_id)
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .await
    }

    async fn set_status(&self, id: GrabId, status: GrabStatus) -> Result<()> {
        let id = id.to_string();
        let status = status_to_str(status)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq("UPDATE grab SET status = ?2 WHERE id = ?1"))
                        .bind(id)
                        .bind(status)
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .await
    }

    async fn set_category(&self, id: GrabId, category: &str) -> Result<()> {
        let id = id.to_string();
        let category = category.to_string();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq("UPDATE grab SET category = ?2 WHERE id = ?1"))
                        .bind(id)
                        .bind(category)
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .await
    }

    async fn delete(&self, id: GrabId) -> Result<bool> {
        // The writer actor's job returns `()`, so detect existence with a read
        // before issuing the delete (the queue-remove path is not hot).
        let existed = sqlx::query(&pq("SELECT 1 FROM grab WHERE id = ?1"))
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .is_some();
        let id = id.to_string();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq("DELETE FROM grab WHERE id = ?1"))
                        .bind(id)
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .await?;
        Ok(existed)
    }
}
