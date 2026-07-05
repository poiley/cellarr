//! Tests for the consistent on-disk snapshot the backup engine bundles.
//!
//! A snapshot must be a self-contained, valid SQLite database that reflects the
//! live data at the moment it was taken — `VACUUM INTO` gives us exactly that.
//!
//! The single-file snapshot is a SQLite-only concept: on the Postgres backend
//! the daemon backs up out-of-band (`pg_dump`) and `Database::open` /
//! `snapshot_to` are not compiled, so this whole file is SQLite-only.
#![cfg(not(feature = "postgres"))]

use cellarr_db::Database;

/// A snapshot of a populated database is a valid, restorable SQLite file that
/// preserves the data, and the live database keeps working afterwards.
#[tokio::test]
async fn snapshot_is_a_valid_copy_of_the_live_data() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("cellarr.sqlite");
    let db = Database::open(db_path.to_str().unwrap()).await.unwrap();

    // Put a row in via a real repo so the snapshot has something to preserve.
    let cfg = db.config();
    let folder = cellarr_core::RootFolder {
        id: uuid::Uuid::new_v4().to_string(),
        path: "/movies".into(),
        name: Some("Movies".into()),
        enabled: true,
    };
    cfg.upsert_root_folder(&folder).await.unwrap();

    // Take the snapshot.
    let snap = dir.path().join("snapshot.sqlite");
    db.snapshot_to(&snap).await.unwrap();
    assert!(snap.exists(), "snapshot file should exist");

    // The live database keeps working after a snapshot.
    cfg.list_root_folders().await.unwrap();
    db.shutdown().await;

    // Open the snapshot as a fresh database and confirm the row is there: the
    // snapshot is a real, independent, restorable copy.
    let restored = Database::open(snap.to_str().unwrap()).await.unwrap();
    let folders = restored.config().list_root_folders().await.unwrap();
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].path, "/movies");
    restored.shutdown().await;
}

/// Refuses to overwrite an existing destination (so a backup never clobbers a
/// sibling file by accident).
#[tokio::test]
async fn snapshot_refuses_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("db.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let dest = dir.path().join("exists.sqlite");
    std::fs::write(&dest, b"not a db").unwrap();
    let err = db.snapshot_to(&dest).await.unwrap_err();
    assert!(matches!(err, cellarr_db::DbError::Backup(_)));
    db.shutdown().await;
}

/// `verify_snapshot` rejects a file that is not a healthy SQLite database.
#[tokio::test]
async fn verify_rejects_garbage() {
    let dir = tempfile::tempdir().unwrap();
    let bogus = dir.path().join("garbage.sqlite");
    std::fs::write(&bogus, b"this is definitely not sqlite").unwrap();
    let err = Database::verify_snapshot(bogus.to_str().unwrap())
        .await
        .unwrap_err();
    assert!(matches!(err, cellarr_db::DbError::Backup(_)));
}
