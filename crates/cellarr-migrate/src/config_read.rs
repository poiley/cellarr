//! Reading the config tables Sonarr and Radarr share: root folders, indexers,
//! and download clients.
//!
//! These tables have the same shape in both apps. Connection settings are
//! carried across verbatim (in the `settings` JSON blob) so the user can re-test
//! them on import; the migration spec is explicit that connections are
//! re-tested, not trusted blind, so cellarr stores rather than validates here.

use cellarr_core::{
    DownloadClientConfig, DownloadClientId, IndexerConfig, IndexerId, Protocol, RootFolder,
};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::error::Result;
use crate::source::{opt_text, SourceKind};

/// Read `RootFolders` rows.
///
/// # Errors
/// Returns a [`crate::MigrationError`] on query failure.
pub(crate) async fn read_root_folders(pool: &SqlitePool) -> Result<Vec<RootFolder>> {
    let rows = sqlx::query("SELECT Id, Path FROM RootFolders ORDER BY Id ASC")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(|row| {
            let id: i64 = row.try_get("Id").unwrap_or(0);
            let path: String = row.try_get("Path").unwrap_or_default();
            RootFolder {
                id: format!("rootfolder-{id}"),
                path,
                name: None,
                enabled: true,
            }
        })
        .collect())
}

/// Read `Indexers` rows. The source's `Implementation` becomes the cellarr
/// adapter `kind`; `Settings` (the JSON connection blob) is carried verbatim.
///
/// # Errors
/// Returns a [`crate::MigrationError`] on query failure.
pub(crate) async fn read_indexers(
    pool: &SqlitePool,
    kind: SourceKind,
) -> Result<Vec<IndexerConfig>> {
    let rows = sqlx::query(
        "SELECT Id, Name, Implementation, Settings, Protocol, Priority, EnableRss
         FROM Indexers ORDER BY Id ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| {
            let name: String = row.try_get("Name").unwrap_or_default();
            let implementation = opt_text(row, "Implementation").unwrap_or_default();
            let settings = parse_settings(opt_text(row, "Settings"));
            let priority: i64 = row.try_get("Priority").unwrap_or(0);
            let enabled = row.try_get::<i64, _>("EnableRss").unwrap_or(1) != 0;
            IndexerConfig {
                id: IndexerId::new(),
                name,
                kind: adapter_kind(&implementation, kind),
                protocol: protocol_of(row),
                enabled,
                priority: priority as i32,
                // The legacy Sonarr/Radarr minimumSeeders/seedCriteria live in the
                // indexer's Settings JSON; carrying them across is long-tail and
                // the criteria default (gate nothing) preserves prior behaviour.
                // TODO(deferred): lift minimumSeeders/seedCriteria/required flags
                // out of the legacy Settings JSON into IndexerCriteria.
                criteria: cellarr_core::IndexerCriteria::default(),
                settings,
            }
        })
        .collect())
}

/// Read `DownloadClients` rows.
///
/// # Errors
/// Returns a [`crate::MigrationError`] on query failure.
pub(crate) async fn read_download_clients(
    pool: &SqlitePool,
    kind: SourceKind,
) -> Result<Vec<DownloadClientConfig>> {
    let rows = sqlx::query(
        "SELECT Id, Name, Implementation, Settings, Protocol, Priority, Enable
         FROM DownloadClients ORDER BY Id ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| {
            let name: String = row.try_get("Name").unwrap_or_default();
            let implementation = opt_text(row, "Implementation").unwrap_or_default();
            let settings = parse_settings(opt_text(row, "Settings"));
            let priority: i64 = row.try_get("Priority").unwrap_or(0);
            let enabled = row.try_get::<i64, _>("Enable").unwrap_or(1) != 0;
            DownloadClientConfig {
                id: DownloadClientId::new(),
                name,
                kind: adapter_kind(&implementation, kind),
                protocol: protocol_of(row),
                enabled,
                priority: priority as i32,
                // The app names its download category per-app; default to the
                // source app name so cellarr keeps touching only its own grabs.
                category: kind.app_name().to_string(),
                settings,
            }
        })
        .collect())
}

/// Map the source `Protocol` column to a cellarr [`Protocol`].
///
/// The originals store either a string (`"torrent"`/`"usenet"`) or an integer
/// enum (0 = usenet, 1 = torrent in the apps' `DownloadProtocol`); handle both.
fn protocol_of(row: &sqlx::sqlite::SqliteRow) -> Protocol {
    if let Some(s) = opt_text(row, "Protocol") {
        return match s.to_ascii_lowercase().as_str() {
            "torrent" => Protocol::Torrent,
            _ => Protocol::Usenet,
        };
    }
    match row.try_get::<i64, _>("Protocol").ok() {
        Some(1) => Protocol::Torrent,
        _ => Protocol::Usenet,
    }
}

/// Parse the source `Settings` JSON blob, defaulting to an empty object.
fn parse_settings(raw: Option<String>) -> serde_json::Value {
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()))
}

/// Lowercase the source `Implementation` into a cellarr adapter kind, prefixing
/// the source app so re-test/round-trip can tell where a config originated.
fn adapter_kind(implementation: &str, _kind: SourceKind) -> String {
    if implementation.is_empty() {
        "unknown".to_string()
    } else {
        implementation.to_ascii_lowercase()
    }
}
