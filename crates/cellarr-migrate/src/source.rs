//! Opening a source *arr database **read-only** and detecting which app it is.
//!
//! Safety is the whole point (docs/12-migration.md): the user's existing install
//! must keep running during evaluation, so the connection is opened in SQLite
//! read-only mode with immutable/`SQLITE_OPEN_READONLY` semantics and we never
//! issue a write. Detection is by *marker tables*: Radarr owns a `Movies` table;
//! Sonarr owns `Series` + `Episodes`. (Lidarr would own `Artists`; it is named
//! here only so detection can report it as recognized-but-unsupported rather than
//! a mysterious failure.)

use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;

use crate::error::{MigrationError, Result};

/// Which *arr application a source database belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Radarr — a movie library.
    Radarr,
    /// Sonarr — a TV (series/season/episode) library.
    Sonarr,
}

impl SourceKind {
    /// The lowercase app name, used in generated config ids and labels.
    #[must_use]
    pub const fn app_name(self) -> &'static str {
        match self {
            SourceKind::Radarr => "radarr",
            SourceKind::Sonarr => "sonarr",
        }
    }
}

/// A read-only handle to a source database, with its detected kind.
pub struct Source {
    pool: SqlitePool,
    kind: SourceKind,
}

impl Source {
    /// Open `path` read-only and detect whether it is Sonarr or Radarr.
    ///
    /// The user's existing app may be running against this same file, so the
    /// connection is strictly read-only (`mode=ro`) and uses a single pooled
    /// connection — we never take a write lock.
    ///
    /// # Errors
    /// Returns [`MigrationError::Source`] if the file cannot be opened and
    /// [`MigrationError::Unrecognized`] if no known marker tables are present.
    pub async fn open(path: &str) -> Result<Self> {
        let pool = open_readonly(path).await?;
        let kind = detect_kind(&pool, path).await?;
        Ok(Self { pool, kind })
    }

    /// The detected source kind.
    #[must_use]
    pub fn kind(&self) -> SourceKind {
        self.kind
    }

    /// The read-only connection pool, for the schema-specific readers.
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

/// Open a SQLite database read-only without touching it.
async fn open_readonly(path: &str) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{path}"))?
        // Never create, never migrate, never write: the user's app owns this file.
        .read_only(true)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    Ok(pool)
}

/// Whether a table exists in the source schema.
async fn table_exists(pool: &SqlitePool, name: &str) -> Result<bool> {
    let row = sqlx::query("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1")
        .bind(name)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Detect the app by marker tables.
async fn detect_kind(pool: &SqlitePool, path: &str) -> Result<SourceKind> {
    // Radarr is identified first by its `Movies` table; Sonarr by the
    // `Series` + `Episodes` pair (a Radarr DB has neither).
    let has_movies = table_exists(pool, "Movies").await?;
    let has_series = table_exists(pool, "Series").await?;
    let has_episodes = table_exists(pool, "Episodes").await?;

    if has_movies && !has_series {
        return Ok(SourceKind::Radarr);
    }
    if has_series && has_episodes {
        return Ok(SourceKind::Sonarr);
    }

    // Recognize Lidarr's marker so the error is informative rather than opaque.
    let lidarr_hint = if table_exists(pool, "Artists").await? {
        " (looks like Lidarr, which is not yet supported)"
    } else {
        ""
    };
    Err(MigrationError::Unrecognized {
        path: path.to_string(),
        detail: format!("no Movies or Series/Episodes marker tables found{lidarr_hint}"),
    })
}

/// Read a nullable `TEXT` column from a row, treating empty strings as absent.
///
/// Source schemas store "no value" inconsistently (NULL or `''`); collapse both
/// so downstream mapping does not have to.
pub(crate) fn opt_text(row: &sqlx::sqlite::SqliteRow, col: &str) -> Option<String> {
    let v: Option<String> = row.try_get(col).ok().flatten();
    v.filter(|s| !s.is_empty())
}
