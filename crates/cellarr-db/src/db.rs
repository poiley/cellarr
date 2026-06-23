//! The [`Database`] handle: opens SQLite, applies migrations, and hands out repos.
//!
//! Readers use the connection pool directly; writers go through the
//! [`crate::writer::WriterHandle`] so all mutation is serialized through one
//! task (docs/08-database.md). The pool is configured for the SQLite single-
//! writer reality: WAL journaling, a nonzero `busy_timeout`, and foreign keys on.

use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::error::Result;
use crate::repos::{
    CacheRepo, ConfigRepo, ContentRepo, DecisionLogRepo, GrabRepo, HistoryRepo, MediaFileRepo,
    ProfileRepo,
};
use crate::writer::WriterHandle;

/// Default bound on the writer channel: enough to absorb normal write bursts
/// without unbounded memory growth.
const DEFAULT_WRITER_BOUND: usize = 256;

/// The application's database handle.
///
/// Cheap to clone where needed via the pool/handle it holds; constructs the
/// concrete repository types on demand. Engine-specific concerns (WAL, the
/// writer-actor) are entirely contained here.
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
    writer: WriterHandle,
}

impl Database {
    /// Open (creating if absent) a SQLite database at `path` and run migrations.
    ///
    /// # Errors
    /// Returns a [`crate::DbError`] if the file cannot be opened or migrations fail.
    pub async fn open(path: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{path}"))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            // Wait, rather than fail immediately, if another connection holds the
            // write lock. The writer-actor makes contention rare, but readers can
            // still race the actor on the WAL.
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);

        Self::connect_with(options).await
    }

    /// Open a private, in-memory database (each handle a fresh schema). Useful
    /// for fast tests; not durable.
    ///
    /// # Errors
    /// Returns a [`crate::DbError`] if the database cannot be created.
    pub async fn open_in_memory() -> Result<Self> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        // An in-memory DB lives only as long as a connection to it; keep the pool
        // pinned to a single connection so schema/data persist across calls.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let writer = WriterHandle::spawn(pool.clone(), DEFAULT_WRITER_BOUND);
        Ok(Self { pool, writer })
    }

    async fn connect_with(options: SqliteConnectOptions) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let writer = WriterHandle::spawn(pool.clone(), DEFAULT_WRITER_BOUND);
        Ok(Self { pool, writer })
    }

    /// The read pool. Repositories use this for queries; callers needing an
    /// escape hatch (e.g. `cellarr-migrate` importing) may borrow it.
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// The shared writer handle.
    #[must_use]
    pub fn writer(&self) -> &WriterHandle {
        &self.writer
    }

    /// The content repository.
    #[must_use]
    pub fn content(&self) -> ContentRepo {
        ContentRepo::new(self.pool.clone(), self.writer.clone())
    }

    /// The media-file repository.
    #[must_use]
    pub fn media_files(&self) -> MediaFileRepo {
        MediaFileRepo::new(self.pool.clone(), self.writer.clone())
    }

    /// The grab repository.
    #[must_use]
    pub fn grabs(&self) -> GrabRepo {
        GrabRepo::new(self.pool.clone(), self.writer.clone())
    }

    /// The history repository.
    #[must_use]
    pub fn history(&self) -> HistoryRepo {
        HistoryRepo::new(self.pool.clone(), self.writer.clone())
    }

    /// The decision-log repository.
    #[must_use]
    pub fn decision_log(&self) -> DecisionLogRepo {
        DecisionLogRepo::new(self.pool.clone(), self.writer.clone())
    }

    /// The quality-profile / custom-format repository.
    #[must_use]
    pub fn profiles(&self) -> ProfileRepo {
        ProfileRepo::new(self.pool.clone(), self.writer.clone())
    }

    /// The cache repository.
    #[must_use]
    pub fn cache(&self) -> CacheRepo {
        CacheRepo::new(self.pool.clone(), self.writer.clone())
    }

    /// The configuration repository (libraries, indexers, clients, …).
    #[must_use]
    pub fn config(&self) -> ConfigRepo {
        ConfigRepo::new(self.pool.clone(), self.writer.clone())
    }
}
