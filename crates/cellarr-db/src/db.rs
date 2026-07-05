//! The [`Database`] handle: opens SQLite, applies migrations, and hands out repos.
//!
//! Readers use the connection pool directly; writers go through the
//! [`crate::writer::WriterHandle`] so all mutation is serialized through one
//! task (docs/08-database.md). The pool is configured for the SQLite single-
//! writer reality: WAL journaling, a nonzero `busy_timeout`, and foreign keys on.

use std::sync::Arc;

#[cfg(not(feature = "postgres"))]
use std::str::FromStr;
#[cfg(not(feature = "postgres"))]
use std::time::Duration;

#[cfg(feature = "postgres")]
use sqlx::postgres::PgPoolOptions;
#[cfg(not(feature = "postgres"))]
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use tokio::sync::Mutex;

use crate::dialect::DbPool;
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
    pool: DbPool,
    writer: WriterHandle,
    // Shared, take-once shutdown control for the writer actor. Behind an `Arc<Mutex<Option<_>>>`
    // so every `Database` clone observes the same control and `shutdown` runs at most once
    // regardless of which clone calls it.
    shutdown: Arc<Mutex<Option<WriterShutdown>>>,
}

impl Database {
    /// Open the database from a connection URL and run migrations, dispatching on
    /// the backend compiled in.
    ///
    /// On the default (SQLite) build the URL is a `sqlite://<path>` (a bare path
    /// is also accepted) and this opens/creates that file. On the `postgres`
    /// build it is a `postgres://…` DSN and this connects to that server. This is
    /// the backend-agnostic entry point the daemon uses; callers pass the
    /// configured target and never branch on engine themselves.
    ///
    /// # Errors
    /// Returns a [`crate::DbError`] if the database cannot be opened/connected or
    /// migrations fail.
    #[cfg(not(feature = "postgres"))]
    pub async fn connect(url: &str) -> Result<Self> {
        // Accept both a `sqlite://path` URL and a bare filesystem path.
        let path = url
            .strip_prefix("sqlite://")
            .or_else(|| url.strip_prefix("sqlite:"))
            .unwrap_or(url);
        Self::open(path).await
    }

    /// Connect to a Postgres server and run migrations. See the SQLite twin.
    ///
    /// # Errors
    /// Returns a [`crate::DbError`] on connect or migration failure.
    #[cfg(feature = "postgres")]
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(url)
            .await?;
        sqlx::migrate!("./migrations/postgres").run(&pool).await?;
        let (writer, shutdown) = WriterHandle::spawn(pool.clone(), DEFAULT_WRITER_BOUND);
        Ok(Self {
            pool,
            writer,
            shutdown: Arc::new(Mutex::new(Some(shutdown))),
        })
    }

    /// Connect to a Postgres server, giving this handle a **private, freshly
    /// migrated `schema`** to work in. Test-only isolation primitive: the caller
    /// passes a unique `schema` name so every test gets a clean namespace on one
    /// shared server, with no cross-test contamination and no dependence on run
    /// order (see the test-support helper).
    ///
    /// The schema is created (idempotently) up front, then every pooled
    /// connection sets its `search_path` to it via `after_connect`, so the
    /// unqualified table names in `migrations/postgres` — and every subsequent
    /// repository query — resolve inside `schema` rather than `public`. The
    /// migration bookkeeping table lands there too, so a fresh schema always runs
    /// the full migration set from empty.
    ///
    /// # Errors
    /// Returns a [`crate::DbError`] on connect, schema creation, or migration
    /// failure.
    #[cfg(feature = "postgres")]
    #[doc(hidden)]
    pub async fn connect_test_schema(url: &str, schema: &str) -> Result<Self> {
        use std::str::FromStr;

        use sqlx::Executor;
        use sqlx::postgres::PgConnectOptions;

        // A schema name is an identifier, not a bind parameter, so it is
        // interpolated into DDL below. The test-support helper only ever passes a
        // generated `test_<n>` name; assert that invariant here rather than open a
        // SQL-injection surface if a caller passes something arbitrary.
        assert!(
            !schema.is_empty()
                && schema
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
            "test schema name must match [a-z0-9_]+, got {schema:?}"
        );

        // Create the schema once on a throwaway connection (still on the default
        // search_path), dropping anything stale so a reused name starts clean.
        {
            let bootstrap = PgPoolOptions::new()
                .max_connections(1)
                .connect(url)
                .await?;
            bootstrap
                .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
                .await?;
            bootstrap
                .execute(format!("CREATE SCHEMA {schema}").as_str())
                .await?;
            bootstrap.close().await;
        }

        // Every connection in the working pool pins its search_path to the new
        // schema, so unqualified DDL/DML resolves there.
        let connect_options = PgConnectOptions::from_str(url)?;
        let set_search_path = format!("SET search_path TO {schema}");
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .after_connect(move |conn, _meta| {
                let stmt = set_search_path.clone();
                Box::pin(async move {
                    conn.execute(stmt.as_str()).await?;
                    Ok(())
                })
            })
            .connect_with(connect_options)
            .await?;

        sqlx::migrate!("./migrations/postgres").run(&pool).await?;
        let (writer, shutdown) = WriterHandle::spawn(pool.clone(), DEFAULT_WRITER_BOUND);
        Ok(Self {
            pool,
            writer,
            shutdown: Arc::new(Mutex::new(Some(shutdown))),
        })
    }

    /// Open (creating if absent) a SQLite database at `path` and run migrations.
    ///
    /// # Errors
    /// Returns a [`crate::DbError`] if the file cannot be opened or migrations fail.
    #[cfg(not(feature = "postgres"))]
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
    #[cfg(not(feature = "postgres"))]
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
        sqlx::migrate!("./migrations/sqlite").run(&pool).await?;
        let (writer, shutdown) = WriterHandle::spawn(pool.clone(), DEFAULT_WRITER_BOUND);
        Ok(Self {
            pool,
            writer,
            shutdown: Arc::new(Mutex::new(Some(shutdown))),
        })
    }

    #[cfg(not(feature = "postgres"))]
    async fn connect_with(options: SqliteConnectOptions) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations/sqlite").run(&pool).await?;
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
    #[cfg(not(feature = "postgres"))]
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

    /// Postgres has no in-process single-file snapshot equivalent of SQLite's
    /// `VACUUM INTO`: a server database is backed up out-of-band (`pg_dump`, the
    /// NAS's own backup of the Postgres data directory), not by the daemon. The
    /// call is kept so the backup engine compiles on both backends; it returns a
    /// clear error rather than pretending to have taken a snapshot.
    ///
    /// # Errors
    /// Always returns [`crate::DbError::Backup`] on the Postgres backend.
    #[cfg(feature = "postgres")]
    pub async fn snapshot_to(&self, _dest: &std::path::Path) -> Result<()> {
        Err(crate::DbError::Backup(
            "in-process database snapshot is SQLite-only; back up Postgres out-of-band (pg_dump)"
                .into(),
        ))
    }

    /// Open `path` read-only and run `PRAGMA integrity_check`, returning an error
    /// if the file is not a healthy SQLite database. Used to validate both a
    /// freshly written snapshot and a candidate restore file before it is trusted.
    ///
    /// # Errors
    /// Returns [`crate::DbError::Backup`] if the file cannot be opened or the
    /// integrity check reports anything other than `ok`.
    #[cfg(not(feature = "postgres"))]
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

    /// Postgres snapshots are validated by the external tooling that produced
    /// them, not by this daemon; the call is kept for backend-agnostic callers
    /// and returns a clear error. See [`snapshot_to`](Self::snapshot_to).
    ///
    /// # Errors
    /// Always returns [`crate::DbError::Backup`] on the Postgres backend.
    #[cfg(feature = "postgres")]
    pub async fn verify_snapshot(path: &str) -> Result<()> {
        Err(crate::DbError::Backup(format!(
            "snapshot verification is SQLite-only; cannot verify {path} on the Postgres backend"
        )))
    }

    /// The read pool. Repositories use this for queries; callers needing an
    /// escape hatch (e.g. `cellarr-migrate` importing) may borrow it.
    #[must_use]
    pub fn pool(&self) -> &DbPool {
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
