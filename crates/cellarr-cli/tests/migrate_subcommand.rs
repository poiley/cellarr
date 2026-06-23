//! The `migrate` subcommand drives `cellarr-migrate` end to end (no longer a
//! `bail!`): a real Radarr fixture DB is imported into a tempdir cellarr DB and
//! the result is verified by reopening that DB.
//!
//! Runs the actual compiled binary (`CARGO_BIN_EXE_cellarr`) so clap parsing, the
//! config layer, and the migrate wiring are all exercised together. The source
//! fixture is copied into a tempdir first so the shared, checked-in fixture is
//! never touched (`docs/16-local-dev-and-testing.md`).

use std::process::Command;

use cellarr_core::repo::ContentRepository;
use cellarr_db::Database;

/// The synthetic, sanitized Radarr fixture maintained by `cellarr-migrate`.
fn source_fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../cellarr-migrate/tests/fixtures/radarr.sqlite")
}

#[tokio::test]
async fn migrate_subcommand_imports_a_fixture_database() {
    let work = tempfile::tempdir().expect("tempdir");

    // Copy the fixture so the import reads a throwaway file, never the shared one.
    let source = work.path().join("radarr.sqlite");
    std::fs::copy(source_fixture(), &source).expect("copy fixture");

    // A separate data dir for the destination cellarr DB.
    let data_dir = work.path().join("data");

    // Invoke the real binary: `cellarr migrate <source>` with the data dir set via
    // env (the same layered config the daemon uses).
    let output = Command::new(env!("CARGO_BIN_EXE_cellarr"))
        .arg("migrate")
        .arg(&source)
        .env("CELLARR_DATA_DIR", &data_dir)
        // Keep the test quiet and deterministic.
        .env("RUST_LOG", "warn")
        .output()
        .expect("run cellarr migrate");

    assert!(
        output.status.success(),
        "migrate exited non-zero: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("imported"),
        "migrate reports an import summary, got: {stdout}"
    );

    // Verify by reopening the destination DB the subcommand created.
    let db_path = data_dir.join("cellarr.sqlite");
    assert!(db_path.exists(), "destination cellarr DB was created");
    let db = Database::open(db_path.to_str().expect("utf-8 path"))
        .await
        .expect("reopen imported DB");

    // The Radarr fixture imports exactly one (movie) library...
    let libs = db.config().list_libraries().await.expect("list libraries");
    assert_eq!(libs.len(), 1, "one movie library imported from Radarr");

    // ...and the file-less movie is a monitored-missing acquisition target,
    // proving content rows (not just config) were written.
    let missing = db
        .content()
        .monitored_missing()
        .await
        .expect("monitored_missing");
    assert_eq!(missing.len(), 1, "the file-less movie imported as missing");

    db.shutdown().await;
}
