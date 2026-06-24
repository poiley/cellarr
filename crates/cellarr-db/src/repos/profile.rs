//! The quality-profile / custom-format repository.

use async_trait::async_trait;
use cellarr_core::repo::ProfileRepository;
use cellarr_core::{CustomFormat, QualityProfile, QualityProfileId};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::error::{DbError, Result};
use crate::writer::WriterHandle;

/// Reads/writes for quality profiles and custom formats.
#[derive(Clone)]
pub struct ProfileRepo {
    pool: SqlitePool,
    writer: WriterHandle,
}

impl ProfileRepo {
    pub(crate) fn new(pool: SqlitePool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }

    /// Insert or replace a quality profile.
    ///
    /// # Errors
    /// Returns a [`DbError`] on serialization or write failure.
    pub async fn upsert_profile(&self, profile: &QualityProfile) -> Result<()> {
        let id = profile.id.to_string();
        let name = profile.name.clone();
        let body = serde_json::to_string(profile)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO quality_profile (id, name, body) VALUES (?1, ?2, ?3)
                         ON CONFLICT(id) DO UPDATE SET name = excluded.name, body = excluded.body",
                    )
                    .bind(id)
                    .bind(name)
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Delete a quality profile by id.
    ///
    /// Idempotent: returns `true` if a row was removed, `false` if no such
    /// profile existed.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn delete_profile(&self, id: QualityProfileId) -> Result<bool> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let id = id.to_string();
        let removed = Arc::new(AtomicBool::new(false));
        let removed_inner = Arc::clone(&removed);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    let result = sqlx::query("DELETE FROM quality_profile WHERE id = ?1")
                        .bind(id)
                        .execute(&mut *conn)
                        .await?;
                    removed_inner.store(result.rows_affected() > 0, Ordering::SeqCst);
                    Ok(())
                })
            })
            .await?;
        Ok(removed.load(Ordering::SeqCst))
    }

    /// Insert or replace a custom format.
    ///
    /// # Errors
    /// Returns a [`DbError`] on serialization or write failure.
    pub async fn upsert_custom_format(&self, format: &CustomFormat) -> Result<()> {
        let id = format.id.to_string();
        let name = format.name.clone();
        let score = i64::from(format.score);
        let body = serde_json::to_string(format)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO custom_format (id, name, score, body) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(id) DO UPDATE SET
                            name = excluded.name, score = excluded.score, body = excluded.body",
                    )
                    .bind(id)
                    .bind(name)
                    .bind(score)
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }
}

#[async_trait]
impl ProfileRepository for ProfileRepo {
    type Error = DbError;

    async fn get_profile(&self, id: QualityProfileId) -> Result<Option<QualityProfile>> {
        let row = sqlx::query("SELECT body FROM quality_profile WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let body: String = row.try_get("body")?;
        Ok(Some(serde_json::from_str(&body)?))
    }

    async fn list_profiles(&self) -> Result<Vec<QualityProfile>> {
        let rows = sqlx::query("SELECT body FROM quality_profile ORDER BY name ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                let body: String = row.try_get("body")?;
                serde_json::from_str(&body).map_err(DbError::from)
            })
            .collect()
    }

    async fn custom_formats(&self) -> Result<Vec<CustomFormat>> {
        let rows = sqlx::query("SELECT body FROM custom_format ORDER BY name ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                let body: String = row.try_get("body")?;
                serde_json::from_str(&body).map_err(DbError::from)
            })
            .collect()
    }
}
