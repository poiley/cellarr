//! The `import_list` repository: import-list configuration and exclusions.
//!
//! Implements [`cellarr_core::ImportListRepository`] over the `import_list` and
//! `import_list_exclusion` tables. Each row's authoritative copy is the serialized
//! JSON in `body`; the columns we filter/order on are mirrored. The
//! `last_successful_sync` timestamp is stamped (in both the typed column and the
//! body) only by [`mark_synced`](ImportListRepo::mark_synced), which the sync job
//! calls **only on a confirmed-good fetch** — the persistence side of the
//! empty-vs-failed safeguard in `cellarr-core::importlist`.

use async_trait::async_trait;
use cellarr_core::importlist::{ImportListConfig, ImportListExclusion, ImportListRepository};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;
use time::OffsetDateTime;

use crate::error::{DbError, Result};
use crate::writer::WriterHandle;

/// Reads/writes for import lists and list exclusions.
#[derive(Clone)]
pub struct ImportListRepo {
    pool: SqlitePool,
    writer: WriterHandle,
}

impl ImportListRepo {
    pub(crate) fn new(pool: SqlitePool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }
}

#[async_trait]
impl ImportListRepository for ImportListRepo {
    type Error = DbError;

    async fn upsert(&self, config: &ImportListConfig) -> Result<()> {
        let id = config.id.clone();
        let name = config.name.clone();
        let kind = config.kind.clone();
        let enabled = i64::from(config.enabled);
        let media_type = media_type_str(config.media_type)?;
        let last_synced = config
            .last_successful_sync
            .map(crate::convert::format_time)
            .transpose()?;
        let body = serde_json::to_string(config)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO import_list
                            (id, name, kind, enabled, media_type, last_synced, body)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         ON CONFLICT(id) DO UPDATE SET
                            name = excluded.name,
                            kind = excluded.kind,
                            enabled = excluded.enabled,
                            media_type = excluded.media_type,
                            last_synced = excluded.last_synced,
                            body = excluded.body",
                    )
                    .bind(id)
                    .bind(name)
                    .bind(kind)
                    .bind(enabled)
                    .bind(media_type)
                    .bind(last_synced)
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    async fn get(&self, id: &str) -> Result<Option<ImportListConfig>> {
        let row = sqlx::query("SELECT body FROM import_list WHERE id = ?1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_json_body).transpose()
    }

    async fn list(&self) -> Result<Vec<ImportListConfig>> {
        let rows = sqlx::query("SELECT body FROM import_list ORDER BY name ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_json_body).collect()
    }

    async fn list_enabled(&self) -> Result<Vec<ImportListConfig>> {
        let rows = sqlx::query("SELECT body FROM import_list WHERE enabled = 1 ORDER BY name ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_json_body).collect()
    }

    async fn delete(&self, id: &str) -> Result<bool> {
        delete_by_id(&self.writer, "import_list", id).await
    }

    async fn mark_synced(&self, id: &str, at: OffsetDateTime) -> Result<()> {
        // Re-read the row so the timestamp lands in BOTH the typed column and the
        // authoritative JSON body (keeping them consistent). A missing row is a
        // no-op (the list was deleted between sync and stamp).
        let Some(mut config) = self.get(id).await? else {
            return Ok(());
        };
        config.last_successful_sync = Some(at);
        self.upsert(&config).await
    }

    async fn upsert_exclusion(&self, exclusion: &ImportListExclusion) -> Result<()> {
        let id = exclusion.id.clone();
        let id_type = exclusion.id_type.clone();
        let id_value = exclusion.id_value.clone();
        let title = exclusion.title.clone();
        let body = serde_json::to_string(exclusion)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO import_list_exclusion
                            (id, id_type, id_value, title, body)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(id_type, id_value) DO UPDATE SET
                            title = excluded.title,
                            body = excluded.body",
                    )
                    .bind(id)
                    .bind(id_type)
                    .bind(id_value)
                    .bind(title)
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    async fn list_exclusions(&self) -> Result<Vec<ImportListExclusion>> {
        let rows = sqlx::query("SELECT body FROM import_list_exclusion ORDER BY title ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_json_body).collect()
    }

    async fn delete_exclusion(&self, id: &str) -> Result<bool> {
        delete_by_id(&self.writer, "import_list_exclusion", id).await
    }
}

/// Delete one row by id from `table`, returning whether a row was removed. The
/// table name is a fixed literal at every call site (never user input).
async fn delete_by_id(writer: &WriterHandle, table: &'static str, id: &str) -> Result<bool> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let id = id.to_string();
    let sql = format!("DELETE FROM {table} WHERE id = ?1");
    let removed = Arc::new(AtomicBool::new(false));
    let removed_inner = Arc::clone(&removed);
    writer
        .submit(move |conn| {
            Box::pin(async move {
                let result = sqlx::query(&sql).bind(id).execute(&mut *conn).await?;
                removed_inner.store(result.rows_affected() > 0, Ordering::SeqCst);
                Ok(())
            })
        })
        .await?;
    Ok(removed.load(Ordering::SeqCst))
}

/// Decode a single `body` JSON column into its typed struct.
fn row_to_json_body<T: serde::de::DeserializeOwned>(row: sqlx::sqlite::SqliteRow) -> Result<T> {
    let body: String = row.try_get("body")?;
    serde_json::from_str(&body).map_err(DbError::from)
}

/// Serialize a [`cellarr_core::MediaType`] to its stored lowercase string.
fn media_type_str(media_type: cellarr_core::MediaType) -> Result<String> {
    Ok(serde_json::to_value(media_type)?
        .as_str()
        .unwrap_or_default()
        .to_string())
}
