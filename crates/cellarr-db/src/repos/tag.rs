//! The persisted tag vocabulary (`/api/v3/tag`).
//!
//! Sonarr/Radarr expose a small `tag` resource (`{ id, label }`) the ecosystem
//! round-trips and tag-scopes routing on. This repo is the persistent backing
//! for that vocabulary: an integer id (the *arr convention; ids start at 1, id 0
//! is never used) and a label, deduplicated case-insensitively. It is the source
//! of truth the content↔tag association and the pipeline's label resolution read
//! against, so tag ids referenced by content and by tag-scoped config survive a
//! restart.

use cellarr_core::Tag;
use crate::dialect::{pq, DbPool};
use sqlx::Row;

use crate::error::Result;
use crate::writer::WriterHandle;

/// Reads/writes for the persisted `tag` vocabulary.
#[derive(Clone)]
pub struct TagRepo {
    pool: DbPool,
    writer: WriterHandle,
}

impl TagRepo {
    pub(crate) fn new(pool: DbPool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }

    /// All tags, ordered by id.
    ///
    /// # Errors
    /// Returns a [`DbError`](crate::DbError) on query/decode failure.
    pub async fn list(&self) -> Result<Vec<Tag>> {
        let rows = sqlx::query(&pq("SELECT id, label FROM tag ORDER BY id ASC"))
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_tag).collect()
    }

    /// One tag by id, or `None` when absent.
    ///
    /// # Errors
    /// Returns a [`DbError`](crate::DbError) on query/decode failure.
    pub async fn get(&self, id: u32) -> Result<Option<Tag>> {
        let row = sqlx::query(&pq("SELECT id, label FROM tag WHERE id = ?1"))
            .bind(i64::from(id))
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_tag).transpose()
    }

    /// Create a tag, returning it with its assigned id. An existing tag with the
    /// same label (case-insensitive) is returned as-is rather than duplicated,
    /// matching the originals' de-duplication. The id is assigned densely from 1
    /// (`MAX(id) + 1`), the *arr convention.
    ///
    /// # Errors
    /// Returns a [`DbError`](crate::DbError) on write failure.
    pub async fn create(&self, label: &str) -> Result<Tag> {
        use std::sync::{Arc, Mutex};
        let label = label.trim().to_string();
        let result: Arc<Mutex<Option<Tag>>> = Arc::new(Mutex::new(None));
        let result_inner = Arc::clone(&result);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    // De-dup case-insensitively: an existing label returns as-is.
                    // SQLite spells this `COLLATE NOCASE`; Postgres has no such
                    // collation, so match on `LOWER(label)` (backed by the
                    // functional unique index).
                    #[cfg(not(feature = "postgres"))]
                    let dedup_sql = "SELECT id, label FROM tag WHERE label = ?1 COLLATE NOCASE";
                    #[cfg(feature = "postgres")]
                    let dedup_sql = "SELECT id, label FROM tag WHERE LOWER(label) = LOWER(?1)";
                    if let Some(row) = sqlx::query(&pq(dedup_sql))
                        .bind(&label)
                        .fetch_optional(&mut *conn)
                        .await?
                    {
                        let id: i64 = row.try_get("id")?;
                        let existing: String = row.try_get("label")?;
                        *result_inner.lock().expect("tag result poisoned") = Some(Tag {
                            id: u32::try_from(id).unwrap_or(0),
                            label: existing,
                        });
                        return Ok(());
                    }
                    // Assign the next dense id (MAX + 1, starting at 1).
                    let next: i64 = sqlx::query(&pq("SELECT COALESCE(MAX(id), 0) + 1 AS next FROM tag"))
                        .fetch_one(&mut *conn)
                        .await?
                        .try_get("next")?;
                    sqlx::query(&pq("INSERT INTO tag (id, label) VALUES (?1, ?2)"))
                        .bind(next)
                        .bind(&label)
                        .execute(&mut *conn)
                        .await?;
                    *result_inner.lock().expect("tag result poisoned") = Some(Tag {
                        id: u32::try_from(next).unwrap_or(0),
                        label,
                    });
                    Ok(())
                })
            })
            .await?;
        let tag = result
            .lock()
            .expect("tag result poisoned")
            .take()
            .expect("create always sets a tag on success");
        Ok(tag)
    }

    /// Update a tag's label. Returns the updated tag, or `None` if absent.
    ///
    /// # Errors
    /// Returns a [`DbError`](crate::DbError) on write failure.
    pub async fn update(&self, id: u32, label: &str) -> Result<Option<Tag>> {
        let label = label.trim().to_string();
        let existed = self.get(id).await?.is_some();
        if !existed {
            return Ok(None);
        }
        let new_label = label.clone();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq("UPDATE tag SET label = ?1 WHERE id = ?2"))
                        .bind(&new_label)
                        .bind(i64::from(id))
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .await?;
        Ok(Some(Tag { id, label }))
    }

    /// Delete a tag. Returns whether it existed. The `content_tag`
    /// `ON DELETE CASCADE` detaches it from every node it tagged.
    ///
    /// # Errors
    /// Returns a [`DbError`](crate::DbError) on write failure.
    pub async fn delete(&self, id: u32) -> Result<bool> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let removed = Arc::new(AtomicBool::new(false));
        let removed_inner = Arc::clone(&removed);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    let result = sqlx::query(&pq("DELETE FROM tag WHERE id = ?1"))
                        .bind(i64::from(id))
                        .execute(&mut *conn)
                        .await?;
                    removed_inner.store(result.rows_affected() > 0, Ordering::SeqCst);
                    Ok(())
                })
            })
            .await?;
        Ok(removed.load(Ordering::SeqCst))
    }

    /// Resolve a set of tag ids to their labels, dropping ids that no longer
    /// exist. Order follows ascending id for determinism.
    ///
    /// # Errors
    /// Returns a [`DbError`](crate::DbError) on query/decode failure.
    pub async fn labels_for(&self, ids: &[u32]) -> Result<Vec<String>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // A small IN-list built from the ids (all integers, no injection risk).
        // Numbered `?N` placeholders (not bare `?`) so `pq` can translate them to
        // `$N` on Postgres; SQLite accepts the numbered form too.
        let placeholders = (1..=ids.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(",");
        // Own the translated SQL: on SQLite `pq` borrows the `format!` temporary,
        // so `.to_string()` keeps it alive for the `q` borrow below.
        let sql =
            pq(&format!("SELECT label FROM tag WHERE id IN ({placeholders}) ORDER BY id ASC"))
                .to_string();
        let mut q = sqlx::query(&sql);
        for id in ids {
            q = q.bind(i64::from(*id));
        }
        let rows = q.fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|r| r.try_get::<String, _>("label").map_err(Into::into))
            .collect()
    }
}

fn row_to_tag(row: crate::dialect::DbRow) -> Result<Tag> {
    let id: i64 = row.try_get("id")?;
    let label: String = row.try_get("label")?;
    Ok(Tag {
        id: u32::try_from(id).unwrap_or(0),
        label,
    })
}
