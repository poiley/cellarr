//! The DB-backed cache table repository.
//!
//! Complements the in-process `moka` caches for values that must survive a
//! restart (docs/08-database.md: no Redis). Entries carry an optional RFC3339
//! expiry; expired rows are treated as absent on read and can be pruned.

use crate::dialect::{pq, DbPool};
use sqlx::Row;
use time::OffsetDateTime;

use crate::convert::format_time;
use crate::error::Result;
use crate::writer::WriterHandle;

/// Reads/writes for the persistent cache table.
#[derive(Clone)]
pub struct CacheRepo {
    pool: DbPool,
    writer: WriterHandle,
}

impl CacheRepo {
    pub(crate) fn new(pool: DbPool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }

    /// Store a value under `key`, optionally expiring at `expires_at`.
    ///
    /// # Errors
    /// Returns a [`crate::DbError`] on write failure.
    pub async fn put(
        &self,
        key: &str,
        value: &str,
        expires_at: Option<OffsetDateTime>,
    ) -> Result<()> {
        let key = key.to_string();
        let value = value.to_string();
        let expires_at = expires_at.map(format_time).transpose()?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq(
                        "INSERT INTO cache (cache_key, value, expires_at) VALUES (?1, ?2, ?3)
                         ON CONFLICT(cache_key) DO UPDATE SET
                            value = excluded.value, expires_at = excluded.expires_at"),
                    )
                    .bind(key)
                    .bind(value)
                    .bind(expires_at)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Fetch a value, treating expired entries as absent.
    ///
    /// # Errors
    /// Returns a [`crate::DbError`] on query failure.
    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        // Compare against now in SQL using string ordering of RFC3339 — only
        // valid because all timestamps are written UTC ('Z') with fixed width.
        let now = format_time(OffsetDateTime::now_utc())?;
        let row = sqlx::query(&pq(
            "SELECT value FROM cache
             WHERE cache_key = ?1 AND (expires_at IS NULL OR expires_at > ?2)"),
        )
        .bind(key)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(r.try_get("value")?)),
            None => Ok(None),
        }
    }

    /// Delete expired entries; returns the number removed.
    ///
    /// # Errors
    /// Returns a [`crate::DbError`] on write failure.
    pub async fn prune_expired(&self) -> Result<()> {
        let now = format_time(OffsetDateTime::now_utc())?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq(
                        "DELETE FROM cache WHERE expires_at IS NOT NULL AND expires_at <= ?1"),
                    )
                    .bind(now)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }
}
