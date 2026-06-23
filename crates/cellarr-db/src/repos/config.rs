//! Configuration repository: libraries and (stubbed) integration config rows.
//!
//! Libraries are first-class structural config (used by content ingest), so they
//! are implemented fully here. Indexers, download clients, root folders, and
//! notifications have schema tables; their typed config structs live in their own
//! crates and are not yet defined, so only the library surface is implemented in
//! this pass. The tables exist so those repos can be filled in without a schema
//! change.

use cellarr_core::{Library, LibraryId, MediaType, QualityProfileId};
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
