//! The managed-config tracking ledger (`managed_config_entity`).
//!
//! This is the persistence backing for config-as-code reconciliation. It records,
//! per `(kind, name)`, which entity the *managed config* previously created — the
//! repo-assigned `entity_id` plus a `content_hash` of the declared item — so the
//! reconciler can classify each declared item as create / update / unchanged and,
//! crucially, prune **only** entities config created (never a UI-created one,
//! which never has a row here).
//!
//! The ledger is deliberately schema-agnostic: `entity_id` is TEXT so one table
//! tracks every kind (uuid, integer, or canonical-name id) uniformly. The
//! reconciler in `cellarr-cli` owns the meaning of `kind`/`name`/`content_hash`;
//! this repo is pure storage.

use crate::dialect::{pq, DbPool};
use sqlx::Row;

use crate::error::Result;
use crate::writer::WriterHandle;

/// One tracked entity: the provenance row config-as-code reconciliation keys on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedEntity {
    /// The managed section/kind (e.g. `"indexer"`, `"quality_profile"`).
    pub kind: String,
    /// The stable human name the config file keys this item by.
    pub name: String,
    /// The concrete repo-assigned id, as text (uuid / integer / canonical name).
    pub entity_id: String,
    /// A stable hash of the declared item, for idempotent change detection.
    pub content_hash: String,
}

/// Reads/writes for the managed-config tracking ledger.
#[derive(Clone)]
pub struct ManagedConfigRepo {
    pool: DbPool,
    writer: WriterHandle,
}

impl ManagedConfigRepo {
    pub(crate) fn new(pool: DbPool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }

    /// Every tracked entity of a given `kind`, ordered by name for determinism.
    ///
    /// This is the "what config previously managed for this section" set the
    /// reconciler diffs the declared items against.
    ///
    /// # Errors
    /// Returns a [`DbError`](crate::DbError) on query/decode failure.
    pub async fn list_kind(&self, kind: &str) -> Result<Vec<ManagedEntity>> {
        let rows = sqlx::query(&pq(
            "SELECT kind, name, entity_id, content_hash
             FROM managed_config_entity WHERE kind = ?1 ORDER BY name ASC"),
        )
        .bind(kind)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_entity).collect()
    }

    /// Every tracked entity across all kinds, ordered by kind then name. Used by
    /// tests and diagnostics that want the whole ledger.
    ///
    /// # Errors
    /// Returns a [`DbError`](crate::DbError) on query/decode failure.
    pub async fn list_all(&self) -> Result<Vec<ManagedEntity>> {
        let rows = sqlx::query(&pq(
            "SELECT kind, name, entity_id, content_hash
             FROM managed_config_entity ORDER BY kind ASC, name ASC"),
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_entity).collect()
    }

    /// Insert or replace a tracking row (the entity config just created/updated).
    ///
    /// # Errors
    /// Returns a [`DbError`](crate::DbError) on write failure.
    pub async fn upsert(&self, entity: &ManagedEntity) -> Result<()> {
        let kind = entity.kind.clone();
        let name = entity.name.clone();
        let entity_id = entity.entity_id.clone();
        let content_hash = entity.content_hash.clone();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(&pq(
                        "INSERT INTO managed_config_entity (kind, name, entity_id, content_hash)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(kind, name) DO UPDATE SET
                            entity_id = excluded.entity_id,
                            content_hash = excluded.content_hash"),
                    )
                    .bind(kind)
                    .bind(name)
                    .bind(entity_id)
                    .bind(content_hash)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Remove a tracking row (the entity config no longer manages, post-prune).
    /// Returns whether a row was removed.
    ///
    /// # Errors
    /// Returns a [`DbError`](crate::DbError) on write failure.
    pub async fn delete(&self, kind: &str, name: &str) -> Result<bool> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let kind = kind.to_string();
        let name = name.to_string();
        let removed = Arc::new(AtomicBool::new(false));
        let removed_inner = Arc::clone(&removed);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    let result = sqlx::query(&pq(
                        "DELETE FROM managed_config_entity WHERE kind = ?1 AND name = ?2"),
                    )
                    .bind(kind)
                    .bind(name)
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

fn row_to_entity(row: crate::dialect::DbRow) -> Result<ManagedEntity> {
    Ok(ManagedEntity {
        kind: row.try_get("kind")?,
        name: row.try_get("name")?,
        entity_id: row.try_get("entity_id")?,
        content_hash: row.try_get("content_hash")?,
    })
}
