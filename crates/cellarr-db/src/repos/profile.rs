//! The quality-profile / custom-format repository.

use async_trait::async_trait;
use cellarr_core::repo::ProfileRepository;
use cellarr_core::{
    CustomFormat, CustomFormatId, DelayProfile, DelayProfileId, QualityDefinition, QualityProfile,
    QualityProfileId, QualityRanking, ReleaseProfile, ReleaseProfileId,
};
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

    /// Fetch one custom format by its id, or `None` if absent.
    ///
    /// # Errors
    /// Returns a [`DbError`] on read or deserialization failure.
    pub async fn get_custom_format(&self, id: CustomFormatId) -> Result<Option<CustomFormat>> {
        let row = sqlx::query("SELECT body FROM custom_format WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let body: String = row.try_get("body")?;
        Ok(Some(serde_json::from_str(&body)?))
    }

    /// Delete a custom format by id.
    ///
    /// Idempotent: returns `true` if a row was removed, `false` if no such format
    /// existed.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn delete_custom_format(&self, id: CustomFormatId) -> Result<bool> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let id = id.to_string();
        let removed = Arc::new(AtomicBool::new(false));
        let removed_inner = Arc::clone(&removed);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    let result = sqlx::query("DELETE FROM custom_format WHERE id = ?1")
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

    /// Insert or replace a delay profile.
    ///
    /// # Errors
    /// Returns a [`DbError`] on serialization or write failure.
    pub async fn upsert_delay_profile(&self, profile: &DelayProfile) -> Result<()> {
        let id = profile.id.to_string();
        let enabled = i64::from(profile.enabled);
        let order = i64::from(profile.order);
        let body = serde_json::to_string(profile)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO delay_profile (id, enabled, \"order\", body) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(id) DO UPDATE SET
                            enabled = excluded.enabled, \"order\" = excluded.\"order\", body = excluded.body",
                    )
                    .bind(id)
                    .bind(enabled)
                    .bind(order)
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Fetch one delay profile by its id, or `None` if absent.
    ///
    /// # Errors
    /// Returns a [`DbError`] on read or deserialization failure.
    pub async fn get_delay_profile(&self, id: DelayProfileId) -> Result<Option<DelayProfile>> {
        let row = sqlx::query("SELECT body FROM delay_profile WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let body: String = row.try_get("body")?;
        Ok(Some(serde_json::from_str(&body)?))
    }

    /// All delay profiles, in resolution order (lowest `order` first), so the
    /// runner can pick the governing profile for a content node.
    ///
    /// # Errors
    /// Returns a [`DbError`] on read or deserialization failure.
    pub async fn list_delay_profiles(&self) -> Result<Vec<DelayProfile>> {
        let rows = sqlx::query("SELECT body FROM delay_profile ORDER BY \"order\" ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                let body: String = row.try_get("body")?;
                serde_json::from_str(&body).map_err(DbError::from)
            })
            .collect()
    }

    /// Persist a per-quality edit (title + size bounds), keyed by the quality's
    /// stable canonical [`QualityDefinition::name`]. The full definition is stored
    /// in `body`; the typed columns mirror the gating fields for cheap inspection.
    ///
    /// This stores an **override** of the code-owned default ranking — only the
    /// editable knobs persist; the catalogue and ranks stay in code. Reload the
    /// merged catalogue with [`Self::quality_ranking`].
    ///
    /// # Errors
    /// Returns a [`DbError`] on serialization or write failure.
    pub async fn upsert_quality_definition(&self, def: &QualityDefinition) -> Result<()> {
        let name = def.name.clone();
        let title = def.title.clone();
        let min = def.min_size_per_min.map(|v| v as i64);
        let max = def.max_size_per_min.map(|v| v as i64);
        let preferred = def.preferred_size_per_min.map(|v| v as i64);
        let body = serde_json::to_string(def)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO quality_definition
                            (name, title, min_size_per_min, max_size_per_min, preferred_size_per_min, body)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(name) DO UPDATE SET
                            title = excluded.title,
                            min_size_per_min = excluded.min_size_per_min,
                            max_size_per_min = excluded.max_size_per_min,
                            preferred_size_per_min = excluded.preferred_size_per_min,
                            body = excluded.body",
                    )
                    .bind(name)
                    .bind(title)
                    .bind(min)
                    .bind(max)
                    .bind(preferred)
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// The persisted per-quality edits, keyed by canonical name.
    ///
    /// # Errors
    /// Returns a [`DbError`] on read or deserialization failure.
    pub async fn quality_definition_overrides(&self) -> Result<Vec<QualityDefinition>> {
        let rows = sqlx::query("SELECT body FROM quality_definition")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                let body: String = row.try_get("body")?;
                serde_json::from_str(&body).map_err(DbError::from)
            })
            .collect()
    }

    /// The effective quality catalogue: the code-owned default ranking with any
    /// persisted per-quality edits (title + size bounds) merged in by canonical
    /// name. The `rank` (and thus all ordering) always comes from the default; an
    /// override row only carries the editable knobs. An override for a name not in
    /// the catalogue is ignored (the catalogue is the source of truth for which
    /// qualities exist).
    ///
    /// This is what the decision engine and the `/api/v3/qualitydefinition` GET
    /// read, so an edit through the PUT is reflected by both.
    ///
    /// # Errors
    /// Returns a [`DbError`] on read or deserialization failure.
    pub async fn quality_ranking(&self) -> Result<QualityRanking> {
        let mut ranking = QualityRanking::default();
        let overrides = self.quality_definition_overrides().await?;
        for ov in overrides {
            if let Some(def) = ranking
                .qualities
                .iter_mut()
                .find(|q| q.name.eq_ignore_ascii_case(&ov.name))
            {
                // Keep the canonical name/rank from the catalogue; take only the
                // editable knobs from the override.
                def.title = ov.title;
                def.min_size_per_min = ov.min_size_per_min;
                def.max_size_per_min = ov.max_size_per_min;
                def.preferred_size_per_min = ov.preferred_size_per_min;
            }
        }
        Ok(ranking)
    }

    /// Delete a delay profile by id.
    ///
    /// Idempotent: returns `true` if a row was removed, `false` otherwise.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn delete_delay_profile(&self, id: DelayProfileId) -> Result<bool> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let id = id.to_string();
        let removed = Arc::new(AtomicBool::new(false));
        let removed_inner = Arc::clone(&removed);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    let result = sqlx::query("DELETE FROM delay_profile WHERE id = ?1")
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

    /// Insert or replace a release profile.
    ///
    /// # Errors
    /// Returns a [`DbError`] on serialization or write failure.
    pub async fn upsert_release_profile(&self, profile: &ReleaseProfile) -> Result<()> {
        let id = profile.id.to_string();
        let enabled = i64::from(profile.enabled);
        let name = profile.name.clone();
        let body = serde_json::to_string(profile)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO release_profile (id, enabled, name, body) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(id) DO UPDATE SET
                            enabled = excluded.enabled, name = excluded.name, body = excluded.body",
                    )
                    .bind(id)
                    .bind(enabled)
                    .bind(name)
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Fetch one release profile by its id, or `None` if absent.
    ///
    /// # Errors
    /// Returns a [`DbError`] on read or deserialization failure.
    pub async fn get_release_profile(
        &self,
        id: ReleaseProfileId,
    ) -> Result<Option<ReleaseProfile>> {
        let row = sqlx::query("SELECT body FROM release_profile WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let body: String = row.try_get("body")?;
        Ok(Some(serde_json::from_str(&body)?))
    }

    /// All release profiles, ordered by name, so the shim can list them and the
    /// decision path can apply every enabled one whose tags match.
    ///
    /// # Errors
    /// Returns a [`DbError`] on read or deserialization failure.
    pub async fn list_release_profiles(&self) -> Result<Vec<ReleaseProfile>> {
        let rows = sqlx::query("SELECT body FROM release_profile ORDER BY name ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                let body: String = row.try_get("body")?;
                serde_json::from_str(&body).map_err(DbError::from)
            })
            .collect()
    }

    /// Delete a release profile by id.
    ///
    /// Idempotent: returns `true` if a row was removed, `false` otherwise.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn delete_release_profile(&self, id: ReleaseProfileId) -> Result<bool> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let id = id.to_string();
        let removed = Arc::new(AtomicBool::new(false));
        let removed_inner = Arc::clone(&removed);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    let result = sqlx::query("DELETE FROM release_profile WHERE id = ?1")
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
