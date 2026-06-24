//! Configuration repository: libraries and the integration config aggregates.
//!
//! Libraries are first-class structural config (used by content ingest). The
//! other config aggregates — [`RootFolder`], [`IndexerConfig`],
//! [`DownloadClientConfig`], [`NotificationConfig`] — now have typed structs in
//! `cellarr-core`. Following docs/02-data-model.md, each is persisted as its
//! serialized JSON in a `body` column, with the few fields we filter/order on
//! mirrored into typed columns; the JSON is the authoritative copy that
//! round-trips losslessly (including the open-ended `settings`).

use cellarr_core::{
    DownloadClientConfig, IndexerConfig, Library, LibraryId, MediaManagement, MediaType,
    NotificationConfig, QualityProfileId, RemotePathMapping, RootFolder,
};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::convert::parse_uuid;
use crate::error::{DbError, Result};
use crate::writer::WriterHandle;

/// Reads/writes for libraries and integration configuration.
#[derive(Clone)]
pub struct ConfigRepo {
    pool: SqlitePool,
    writer: WriterHandle,
}

impl ConfigRepo {
    pub(crate) fn new(pool: SqlitePool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }

    /// Insert or replace a library.
    ///
    /// # Errors
    /// Returns a [`DbError`] on serialization or write failure.
    pub async fn upsert_library(&self, library: &Library) -> Result<()> {
        let id = library.id.to_string();
        let media_type = serde_json::to_value(library.media_type)?
            .as_str()
            .unwrap_or_default()
            .to_string();
        let name = library.name.clone();
        let root_folders = serde_json::to_string(&library.root_folders)?;
        let default_profile = library.default_quality_profile.to_string();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO library
                            (id, media_type, name, root_folders, default_quality_profile)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(id) DO UPDATE SET
                            media_type = excluded.media_type,
                            name = excluded.name,
                            root_folders = excluded.root_folders,
                            default_quality_profile = excluded.default_quality_profile",
                    )
                    .bind(id)
                    .bind(media_type)
                    .bind(name)
                    .bind(root_folders)
                    .bind(default_profile)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Fetch a library by id.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn get_library(&self, id: LibraryId) -> Result<Option<Library>> {
        let row = sqlx::query(
            "SELECT id, media_type, name, root_folders, default_quality_profile
             FROM library WHERE id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_library).transpose()
    }

    /// All libraries, by name.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn list_libraries(&self) -> Result<Vec<Library>> {
        let rows = sqlx::query(
            "SELECT id, media_type, name, root_folders, default_quality_profile
             FROM library ORDER BY name ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_library).collect()
    }

    // --- Root folders -------------------------------------------------------

    /// Insert or replace a root folder.
    ///
    /// # Errors
    /// Returns a [`DbError`] on serialization or write failure.
    pub async fn upsert_root_folder(&self, folder: &RootFolder) -> Result<()> {
        let id = folder.id.clone();
        let path = folder.path.clone();
        let name = folder.name.clone();
        let enabled = i64::from(folder.enabled);
        let body = serde_json::to_string(folder)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO root_folder (id, path, name, enabled, body)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(id) DO UPDATE SET
                            path = excluded.path,
                            name = excluded.name,
                            enabled = excluded.enabled,
                            body = excluded.body",
                    )
                    .bind(id)
                    .bind(path)
                    .bind(name)
                    .bind(enabled)
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Fetch a root folder by id.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn get_root_folder(&self, id: &str) -> Result<Option<RootFolder>> {
        let row = sqlx::query("SELECT body FROM root_folder WHERE id = ?1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_json_body).transpose()
    }

    /// All root folders, by path.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn list_root_folders(&self) -> Result<Vec<RootFolder>> {
        let rows = sqlx::query("SELECT body FROM root_folder ORDER BY path ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_json_body).collect()
    }

    /// Delete a root folder by id. Returns whether a row was removed (so an
    /// already-deleted id can be reported as the idempotent no-op the `/api/v3`
    /// shim treats as success).
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn delete_root_folder(&self, id: &str) -> Result<bool> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let id = id.to_string();
        let removed = Arc::new(AtomicBool::new(false));
        let removed_inner = Arc::clone(&removed);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    let result = sqlx::query("DELETE FROM root_folder WHERE id = ?1")
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

    // --- Indexers -----------------------------------------------------------

    /// Insert or replace an indexer configuration.
    ///
    /// # Errors
    /// Returns a [`DbError`] on serialization or write failure.
    pub async fn upsert_indexer(&self, indexer: &IndexerConfig) -> Result<()> {
        let id = indexer.id.to_string();
        let name = indexer.name.clone();
        let kind = indexer.kind.clone();
        let protocol = protocol_str(indexer.protocol)?;
        let enabled = i64::from(indexer.enabled);
        let priority = i64::from(indexer.priority);
        let body = serde_json::to_string(indexer)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO indexer
                            (id, name, kind, protocol, enabled, priority, body)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         ON CONFLICT(id) DO UPDATE SET
                            name = excluded.name,
                            kind = excluded.kind,
                            protocol = excluded.protocol,
                            enabled = excluded.enabled,
                            priority = excluded.priority,
                            body = excluded.body",
                    )
                    .bind(id)
                    .bind(name)
                    .bind(kind)
                    .bind(protocol)
                    .bind(enabled)
                    .bind(priority)
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Fetch an indexer configuration by id.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn get_indexer(&self, id: cellarr_core::IndexerId) -> Result<Option<IndexerConfig>> {
        let row = sqlx::query("SELECT body FROM indexer WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_json_body).transpose()
    }

    /// All indexer configurations, by name.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn list_indexers(&self) -> Result<Vec<IndexerConfig>> {
        let rows = sqlx::query("SELECT body FROM indexer ORDER BY name ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_json_body).collect()
    }

    /// All *enabled* indexer configurations, ordered by ascending priority (the
    /// *arr convention: lower priority is preferred), then name. This is the set
    /// the discovery pipeline fans a search across.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn list_enabled_indexers(&self) -> Result<Vec<IndexerConfig>> {
        let rows = sqlx::query(
            "SELECT body FROM indexer WHERE enabled = 1 ORDER BY priority ASC, name ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_json_body).collect()
    }

    /// Delete an indexer configuration by id. Idempotent: returns `true` if a row
    /// was removed, `false` if no such indexer existed.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn delete_indexer(&self, id: cellarr_core::IndexerId) -> Result<bool> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let id = id.to_string();
        let removed = Arc::new(AtomicBool::new(false));
        let removed_inner = Arc::clone(&removed);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    let result = sqlx::query("DELETE FROM indexer WHERE id = ?1")
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

    // --- Download clients ---------------------------------------------------

    /// Insert or replace a download-client configuration.
    ///
    /// # Errors
    /// Returns a [`DbError`] on serialization or write failure.
    pub async fn upsert_download_client(&self, client: &DownloadClientConfig) -> Result<()> {
        let id = client.id.to_string();
        let name = client.name.clone();
        let kind = client.kind.clone();
        let protocol = protocol_str(client.protocol)?;
        let enabled = i64::from(client.enabled);
        let priority = i64::from(client.priority);
        let category = client.category.clone();
        let body = serde_json::to_string(client)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO download_client
                            (id, name, kind, protocol, enabled, priority, category, body)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                         ON CONFLICT(id) DO UPDATE SET
                            name = excluded.name,
                            kind = excluded.kind,
                            protocol = excluded.protocol,
                            enabled = excluded.enabled,
                            priority = excluded.priority,
                            category = excluded.category,
                            body = excluded.body",
                    )
                    .bind(id)
                    .bind(name)
                    .bind(kind)
                    .bind(protocol)
                    .bind(enabled)
                    .bind(priority)
                    .bind(category)
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Fetch a download-client configuration by id.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn get_download_client(
        &self,
        id: cellarr_core::DownloadClientId,
    ) -> Result<Option<DownloadClientConfig>> {
        let row = sqlx::query("SELECT body FROM download_client WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_json_body).transpose()
    }

    /// All download-client configurations, by name.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn list_download_clients(&self) -> Result<Vec<DownloadClientConfig>> {
        let rows = sqlx::query("SELECT body FROM download_client ORDER BY name ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_json_body).collect()
    }

    /// Delete a download-client configuration by id. Idempotent: returns `true`
    /// if a row was removed, `false` if no such client existed.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn delete_download_client(&self, id: cellarr_core::DownloadClientId) -> Result<bool> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let id = id.to_string();
        let removed = Arc::new(AtomicBool::new(false));
        let removed_inner = Arc::clone(&removed);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    let result = sqlx::query("DELETE FROM download_client WHERE id = ?1")
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

    // --- Notifications ------------------------------------------------------

    /// Insert or replace a notification configuration.
    ///
    /// # Errors
    /// Returns a [`DbError`] on serialization or write failure.
    pub async fn upsert_notification(&self, notification: &NotificationConfig) -> Result<()> {
        let id = notification.id.clone();
        let name = notification.name.clone();
        let kind = notification.kind.clone();
        let enabled = i64::from(notification.enabled);
        let body = serde_json::to_string(notification)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO notification (id, name, kind, enabled, body)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(id) DO UPDATE SET
                            name = excluded.name,
                            kind = excluded.kind,
                            enabled = excluded.enabled,
                            body = excluded.body",
                    )
                    .bind(id)
                    .bind(name)
                    .bind(kind)
                    .bind(enabled)
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Fetch a notification configuration by id.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn get_notification(&self, id: &str) -> Result<Option<NotificationConfig>> {
        let row = sqlx::query("SELECT body FROM notification WHERE id = ?1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_json_body).transpose()
    }

    /// All notification configurations, by name.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn list_notifications(&self) -> Result<Vec<NotificationConfig>> {
        let rows = sqlx::query("SELECT body FROM notification ORDER BY name ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_json_body).collect()
    }

    /// The *enabled* notification configurations of a given `kind`, by name. The
    /// per-kind index ([`idx_notification_kind`]) backs this so the dispatcher's
    /// per-provider fan-out never scans every configured notification.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn list_enabled_notifications_by_kind(
        &self,
        kind: &str,
    ) -> Result<Vec<NotificationConfig>> {
        let rows = sqlx::query(
            "SELECT body FROM notification
             WHERE enabled = 1 AND kind = ?1 ORDER BY name ASC",
        )
        .bind(kind)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_json_body).collect()
    }

    /// Delete a notification configuration by id. Idempotent: returns `true` if a
    /// row was removed, `false` if no such notification existed (so the `/api/v3`
    /// shim can report the idempotent-success the *arr clients expect).
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn delete_notification(&self, id: &str) -> Result<bool> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let id = id.to_string();
        let removed = Arc::new(AtomicBool::new(false));
        let removed_inner = Arc::clone(&removed);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    let result = sqlx::query("DELETE FROM notification WHERE id = ?1")
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

    // --- Remote-path mappings ----------------------------------------------

    /// Insert or replace a remote-path mapping.
    ///
    /// # Errors
    /// Returns a [`DbError`] on serialization or write failure.
    pub async fn upsert_remote_path_mapping(&self, mapping: &RemotePathMapping) -> Result<()> {
        let id = mapping.id.clone();
        let host = mapping.host.clone();
        let remote_path = mapping.remote_path.clone();
        let local_path = mapping.local_path.clone();
        let body = serde_json::to_string(mapping)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO remote_path_mapping
                            (id, host, remote_path, local_path, body)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(id) DO UPDATE SET
                            host = excluded.host,
                            remote_path = excluded.remote_path,
                            local_path = excluded.local_path,
                            body = excluded.body",
                    )
                    .bind(id)
                    .bind(host)
                    .bind(remote_path)
                    .bind(local_path)
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }

    /// Fetch a remote-path mapping by id.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn get_remote_path_mapping(&self, id: &str) -> Result<Option<RemotePathMapping>> {
        let row = sqlx::query("SELECT body FROM remote_path_mapping WHERE id = ?1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(row_to_json_body).transpose()
    }

    /// All remote-path mappings, by host then remote path (a stable order the
    /// shared apply step iterates in).
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn list_remote_path_mappings(&self) -> Result<Vec<RemotePathMapping>> {
        let rows =
            sqlx::query("SELECT body FROM remote_path_mapping ORDER BY host ASC, remote_path ASC")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter().map(row_to_json_body).collect()
    }

    /// Delete a remote-path mapping by id. Idempotent: returns `true` if a row
    /// was removed, `false` if no such mapping existed.
    ///
    /// # Errors
    /// Returns a [`DbError`] on write failure.
    pub async fn delete_remote_path_mapping(&self, id: &str) -> Result<bool> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let id = id.to_string();
        let removed = Arc::new(AtomicBool::new(false));
        let removed_inner = Arc::clone(&removed);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    let result = sqlx::query("DELETE FROM remote_path_mapping WHERE id = ?1")
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

    /// The library-wide media-management settings (recycle bin, naming formats,
    /// permission policy, extra-file import). Returns [`MediaManagement::default`]
    /// when no row has been written yet, so a zero-config library behaves exactly
    /// as it did before these settings were persistable.
    ///
    /// # Errors
    /// Returns a [`DbError`] on query/decode failure.
    pub async fn get_media_management(&self) -> Result<MediaManagement> {
        let row = sqlx::query("SELECT body FROM media_management WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => row_to_json_body(row),
            None => Ok(MediaManagement::default()),
        }
    }

    /// Persist the library-wide media-management settings, replacing the single
    /// settings document in place.
    ///
    /// # Errors
    /// Returns a [`DbError`] on serialization or write failure.
    pub async fn set_media_management(&self, settings: &MediaManagement) -> Result<()> {
        let body = serde_json::to_string(settings)?;
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO media_management (id, body) VALUES (1, ?1)
                         ON CONFLICT(id) DO UPDATE SET body = excluded.body",
                    )
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await
    }
}

/// Decode a single `body` JSON column into its typed config struct.
fn row_to_json_body<T: serde::de::DeserializeOwned>(row: sqlx::sqlite::SqliteRow) -> Result<T> {
    let body: String = row.try_get("body")?;
    serde_json::from_str(&body).map_err(DbError::from)
}

/// Serialize a [`cellarr_core::Protocol`] to its stored lowercase string.
fn protocol_str(protocol: cellarr_core::Protocol) -> Result<String> {
    Ok(serde_json::to_value(protocol)?
        .as_str()
        .unwrap_or_default()
        .to_string())
}

fn row_to_library(row: sqlx::sqlite::SqliteRow) -> Result<Library> {
    let id: String = row.try_get("id")?;
    let media_type: String = row.try_get("media_type")?;
    let name: String = row.try_get("name")?;
    let root_folders: String = row.try_get("root_folders")?;
    let default_profile: String = row.try_get("default_quality_profile")?;

    let media_type: MediaType =
        serde_json::from_value(serde_json::Value::String(media_type)).map_err(DbError::from)?;
    let root_folders: Vec<String> = serde_json::from_str(&root_folders)?;

    Ok(Library {
        id: LibraryId::from_uuid(parse_uuid("id", &id)?),
        media_type,
        name,
        root_folders,
        default_quality_profile: QualityProfileId::from_uuid(parse_uuid(
            "default_quality_profile",
            &default_profile,
        )?),
    })
}
