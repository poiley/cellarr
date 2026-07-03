//! The [`Database`] handle: opens SQLite, applies migrations, and hands out repos.
//!
//! Readers use the connection pool directly; writers go through the
//! [`crate::writer::WriterHandle`] so all mutation is serialized through one
//! task (docs/08-database.md). The pool is configured for the SQLite single-
//! writer reality: WAL journaling, a nonzero `busy_timeout`, and foreign keys on.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use tokio::sync::Mutex;

use crate::error::Result;
use crate::repos::{
    AuthRepo, BlocklistRepo, CacheRepo, ConfigRepo, ContentRepo, DecisionLogRepo, GrabRepo,
    HistoryRepo, ImportListRepo, ManagedConfigRepo, MediaFileRepo, PendingReleaseRepo, ProfileRepo,
    TagRepo,
};
use crate::writer::{WriterHandle, WriterShutdown};

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
    // Shared, take-once shutdown control for the writer actor. Behind an `Arc<Mutex<Option<_>>>`
    // so every `Database` clone observes the same control and `shutdown` runs at most once
    // regardless of which clone calls it.
    shutdown: Arc<Mutex<Option<WriterShutdown>>>,
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
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            // Give each connection a page cache large enough to hold the whole
            // working set in memory. The database file can live on high-latency
            // storage (a homelab NAS mount reaches it over CIFS), where every
            // uncached page read is a network round-trip; a 2 MB default cache
            // turned list endpoints into thousands of round-trips and multi-second
            // loads. A generous cache means a connection reads each page from the
            // network at most once, then serves it from RAM.
            .pragma("cache_size", "-65536")
            // Keep sort/temp b-trees (ORDER BY, GROUP BY spills) in memory rather
            // than materializing them onto the (possibly networked) DB directory.
            .pragma("temp_store", "MEMORY");

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
        let (writer, shutdown) = WriterHandle::spawn(pool.clone(), DEFAULT_WRITER_BOUND);
        Ok(Self {
            pool,
            writer,
            shutdown: Arc::new(Mutex::new(Some(shutdown))),
        })
    }

    async fn connect_with(options: SqliteConnectOptions) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let (writer, shutdown) = WriterHandle::spawn(pool.clone(), DEFAULT_WRITER_BOUND);
        Ok(Self {
            pool,
            writer,
            shutdown: Arc::new(Mutex::new(Some(shutdown))),
        })
    }

    /// Shut the database down cleanly: stop the writer actor (so it releases its
    /// dedicated connection), then close the pool — leaving a consistent DB with
    /// no half-applied transaction. Safe to call on any clone and more than once
    /// (only the first call does the work).
    ///
    /// This is the correct teardown; **do not** call `pool().close()` directly
    /// while a `Database` is alive — the writer actor holds a connection the pool
    /// would wait on forever (a deadlock).
    pub async fn shutdown(&self) {
        if let Some(ctrl) = self.shutdown.lock().await.take() {
            ctrl.shutdown().await;
        }
        self.pool.close().await;
    }

    /// Write a **consistent on-disk snapshot** of the live database to `dest`.
    ///
    /// Uses SQLite's `VACUUM INTO`, which produces a transactionally-consistent,
    /// fully-compacted copy of the database as a single self-contained file — no
    /// WAL/`-shm` sidecars, no torn pages — without taking the database offline or
    /// blocking the writer-actor for the duration. This is the snapshot the backup
    /// engine bundles (`docs/08-database.md`: the writer-actor serializes writes,
    /// so the read pool can run `VACUUM INTO` against a consistent view).
    ///
    /// `dest` must not already exist (SQLite refuses to overwrite), and its parent
    /// directory must exist. After the copy the snapshot is reopened and
    /// `PRAGMA integrity_check` is run so a corrupt or truncated file is caught
    /// here rather than at restore time.
    ///
    /// # Errors
    /// Returns [`crate::DbError::Backup`] if `dest` is unusable (already exists,
    /// not valid UTF-8) or the integrity check fails, or the underlying sqlx error
    /// if `VACUUM INTO` itself fails.
    pub async fn snapshot_to(&self, dest: &std::path::Path) -> Result<()> {
        if dest.exists() {
            return Err(crate::DbError::Backup(format!(
                "snapshot destination already exists: {}",
                dest.display()
            )));
        }
        let dest_str = dest.to_str().ok_or_else(|| {
            crate::DbError::Backup(format!(
                "snapshot destination is not valid UTF-8: {}",
                dest.display()
            ))
        })?;
        // `VACUUM INTO` binds no parameters; the path is a SQL string literal, so
        // we reject embedded quotes outright rather than attempt escaping (a backup
        // path with a quote in it is pathological and not worth a SQL-injection
        // surface). Timestamped backup names never contain quotes.
        if dest_str.contains('\'') {
            return Err(crate::DbError::Backup(
                "snapshot destination path contains a single quote".into(),
            ));
        }
        sqlx::query(&format!("VACUUM INTO '{dest_str}'"))
            .execute(&self.pool)
            .await?;

        // Reopen the snapshot read-only and verify integrity, so a silently
        // corrupt copy never ships as a "valid" backup.
        Self::verify_snapshot(dest_str).await
    }

    /// Open `path` read-only and run `PRAGMA integrity_check`, returning an error
    /// if the file is not a healthy SQLite database. Used to validate both a
    /// freshly written snapshot and a candidate restore file before it is trusted.
    ///
    /// # Errors
    /// Returns [`crate::DbError::Backup`] if the file cannot be opened or the
    /// integrity check reports anything other than `ok`.
    pub async fn verify_snapshot(path: &str) -> Result<()> {
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{path}"))
            .map_err(|e| crate::DbError::Backup(format!("opening snapshot {path}: {e}")))?
            .read_only(true)
            .create_if_missing(false);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|e| crate::DbError::Backup(format!("opening snapshot {path}: {e}")))?;
        let row: (String,) = sqlx::query_as("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .map_err(|e| crate::DbError::Backup(format!("integrity check on {path}: {e}")))?;
        pool.close().await;
        if row.0 == "ok" {
            Ok(())
        } else {
            Err(crate::DbError::Backup(format!(
                "snapshot {path} failed integrity check: {}",
                row.0
            )))
        }
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

    /// The pending-release repository (delay-profile first-seen bookkeeping).
    #[must_use]
    pub fn pending_releases(&self) -> PendingReleaseRepo {
        PendingReleaseRepo::new(self.pool.clone(), self.writer.clone())
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

    /// The persisted tag-vocabulary repository (`/api/v3/tag`).
    #[must_use]
    pub fn tags(&self) -> TagRepo {
        TagRepo::new(self.pool.clone(), self.writer.clone())
    }

    /// The managed-config tracking ledger (config-as-code reconciliation).
    #[must_use]
    pub fn managed_config(&self) -> ManagedConfigRepo {
        ManagedConfigRepo::new(self.pool.clone(), self.writer.clone())
    }

    /// The failed-download blocklist repository.
    #[must_use]
    pub fn blocklist(&self) -> BlocklistRepo {
        BlocklistRepo::new(self.pool.clone(), self.writer.clone())
    }

    /// The import-list repository (lists + exclusions).
    #[must_use]
    pub fn import_lists(&self) -> ImportListRepo {
        ImportListRepo::new(self.pool.clone(), self.writer.clone())
    }

    /// The authentication repository (single-admin auth config + Forms sessions).
    #[must_use]
    pub fn auth(&self) -> AuthRepo {
        AuthRepo::new(self.pool.clone(), self.writer.clone())
    }
}
