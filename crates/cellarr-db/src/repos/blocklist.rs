//! The `blocklist` repository: failed-download blocklist persistence.
//!
//! Implements [`cellarr_core::BlocklistRepository`] over the `blocklist` table.
//! An entry's authoritative copy is the serialized JSON in `body`; the columns we
//! filter/order on are mirrored. `add` is idempotent on `(content_id,
//! release_key)` so re-blocklisting the same release refreshes the row.

use async_trait::async_trait;
use cellarr_core::blocklist::{release_key, BlocklistEntry, BlocklistRepository};
use cellarr_core::{ContentId, Release};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::error::{DbError, Result};
use crate::writer::WriterHandle;

/// Reads/writes for the failed-download blocklist.
#[derive(Clone)]
pub struct BlocklistRepo {
    pool: SqlitePool,
    writer: WriterHandle,
}

impl BlocklistRepo {
    pub(crate) fn new(pool: SqlitePool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }
}

#[async_trait]
impl BlocklistRepository for BlocklistRepo {
    type Error = DbError;

    async fn add(&self, entry: &BlocklistEntry) -> Result<()> {
        let id = entry.id.clone();
        let content_id = entry.content_id.to_string();
        let key = entry.release_key.clone();
        let title = entry.title.clone();
        let reason = entry.reason.clone();
        let at = crate::convert::format_time(entry.blocklisted_at)?;
        let body = serde_json::to_string(entry)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    // Idempotent on (content_id, release_key): a repeated failure
                    // for the same release refreshes the reason/time, never
                    // duplicates the row.
                    sqlx::query(
                        "INSERT INTO blocklist
                            (id, content_id, release_key, title, reason, blocklisted_at, body)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         ON CONFLICT(content_id, release_key) DO UPDATE SET
                            title = excluded.title,
                            reason = excluded.reason,
                            blocklisted_at = excluded.blocklisted_at,
                            body = excluded.body",
                    )
                    .bind(id)
                    .bind(content_id)
                    .bind(key)
                    .bind(title)
                    .bind(reason)
                    .bind(at)
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    async fn is_blocklisted(&self, content_id: ContentId, release: &Release) -> Result<bool> {
        let key = release_key(release);
        let row = sqlx::query(
            "SELECT 1 FROM blocklist WHERE content_id = ?1 AND release_key = ?2 LIMIT 1",
        )
        .bind(content_id.to_string())
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    async fn list(&self) -> Result<Vec<BlocklistEntry>> {
        let rows = sqlx::query("SELECT body FROM blocklist ORDER BY blocklisted_at DESC, id ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                let body: String = row.try_get("body")?;
                serde_json::from_str(&body).map_err(DbError::from)
            })
            .collect()
    }

    async fn remove(&self, id: &str) -> Result<bool> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let id = id.to_string();
        let removed = Arc::new(AtomicBool::new(false));
        let removed_inner = Arc::clone(&removed);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    let result = sqlx::query("DELETE FROM blocklist WHERE id = ?1")
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
