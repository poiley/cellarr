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
    DownloadClientConfig, IndexerConfig, Library, LibraryId, MediaType, NotificationConfig,
    QualityProfileId, RootFolder,
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
