//! Shared test-support for the cellarr-db integration tests.
//!
//! [`test_database`] hands each test a fresh, fully-migrated [`Database`] on
//! whichever backend the crate was compiled with, keeping the tests themselves
//! backend-agnostic.
//!
//! # Isolation strategy
//!
//! * **SQLite (default build).** Each call opens a brand-new database file under
//!   its own `tempfile` directory, so tests never share state. The temp dir is
//!   deliberately *leaked* (its path is kept, not RAII-dropped) so the file
//!   outlives the returned handle for the whole test — the `Database` owns no
//!   reference back to the dir. These files are small and land in the OS temp
//!   area; the process is short-lived, so leaking them is fine for a test run.
//!
//! * **Postgres (`--features postgres`).** All tests run against the one shared
//!   server at `$CELLARR_TEST_DATABASE_URL`, so isolation is per **schema**:
//!   each call mints a unique `test_<n>` schema name, and
//!   [`Database::connect_test_schema`] drops-and-recreates that schema, points
//!   the pool's `search_path` at it, and migrates into it. Because every test
//!   gets its own empty namespace and the schema name is unique per call, there
//!   is no cross-test contamination and no dependence on run order — tests can
//!   run in parallel exactly as they do on SQLite. The uniqueness comes from a
//!   process-global counter combined with the process id, so two test binaries
//!   pointed at the same server (unusual, but harmless) still never collide.

use cellarr_db::Database;

/// Open a fresh, migrated database for a single test on the compiled backend.
///
/// See the module docs for the per-backend isolation strategy. Panics on any
/// setup failure — a test that cannot get a database has nothing to assert.
pub async fn test_database() -> Database {
    #[cfg(not(feature = "postgres"))]
    {
        // A private on-disk SQLite file per test. Leak the temp dir so the file
        // lives as long as the returned handle (the `Database` holds no handle
        // back to the dir).
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.keep();
        let db_path = path.join("cellarr.db");
        Database::open(db_path.to_str().expect("utf8 path"))
            .await
            .expect("open + migrate sqlite test db")
    }

    #[cfg(feature = "postgres")]
    {
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let url = std::env::var("CELLARR_TEST_DATABASE_URL").expect(
            "CELLARR_TEST_DATABASE_URL must be set for Postgres tests \
             (see `just test-pg`, which starts an ephemeral Postgres and exports it)",
        );
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let schema = format!("test_{}_{}", std::process::id(), n);
        Database::connect_test_schema(&url, &schema)
            .await
            .expect("connect + migrate postgres test schema")
    }
}
