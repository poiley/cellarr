//! `/api/v3` integration tests for the backup, log-file, and expanded-health
//! surfaces. These exercise the real router end to end over HTTP, with the
//! backup engine + log reader wired into [`AppState`] against a file-backed DB
//! under a temp dir (so the atomic restore swap has a real file to operate on).

use std::path::PathBuf;

use cellarr_api::{AppState, AuthConfig, BackupEngine, LogFiles};
use cellarr_db::Database;

/// A test server with the backup + log surfaces wired, exposing the temp dir so a
/// test can write log files and inspect the on-disk backups.
struct Server {
    base_url: String,
    state: AppState,
    db_path: PathBuf,
    log_dir: PathBuf,
    _dir: tempfile::TempDir,
    _handle: tokio::task::JoinHandle<()>,
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("cellarr.sqlite");
    let backup_dir = dir.path().join("backups");
    let log_dir = dir.path().join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();

    let db = Database::open(db_path.to_str().unwrap()).await.unwrap();
    let engine = BackupEngine::new(backup_dir, db.clone(), db_path.clone());
    let state = AppState::new(db, AuthConfig::disabled())
        .with_backup(engine)
        .with_log_files(LogFiles::new(log_dir.clone()));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    let serve_state = state.clone();
    let handle = tokio::spawn(async move {
        let _ = cellarr_api::serve(listener, serve_state).await;
    });
    Server {
        base_url,
        state,
        db_path,
        log_dir,
        _dir: dir,
        _handle: handle,
    }
}

impl Server {
    fn url(&self, p: &str) -> String {
        format!("{}{p}", self.base_url)
    }
}

// --- backup ----------------------------------------------------------------

#[tokio::test]
async fn backup_create_list_download_delete_round_trip() {
    let srv = start().await;
    let client = reqwest::Client::new();

    // Initially empty.
    let list: serde_json::Value = client
        .get(srv.url("/api/v3/system/backup"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 0);

    // Create a manual backup.
    let created: serde_json::Value = client
        .post(srv.url("/api/v3/system/backup"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let backup_id = created["backupId"].as_str().unwrap().to_string();
    assert_eq!(created["type"], "manual");
    assert!(created["size"].as_u64().unwrap() > 0);

    // Now it lists.
    let list: serde_json::Value = client
        .get(srv.url("/api/v3/system/backup"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Download returns the raw bundle bytes.
    let bytes = client
        .get(srv.url(&format!("/api/v3/system/backup/{backup_id}")))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert!(bytes.starts_with(b"CELLARRBKP"));

    // Delete removes it (idempotent).
    let status = client
        .delete(srv.url(&format!("/api/v3/system/backup/{backup_id}")))
        .send()
        .await
        .unwrap()
        .status();
    assert!(status.is_success());
    let list: serde_json::Value = client
        .get(srv.url("/api/v3/system/backup"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn restore_round_trips_to_backed_up_state_over_http() {
    let srv = start().await;
    let client = reqwest::Client::new();

    // Original state: one root folder.
    let folder = cellarr_core::RootFolder {
        id: uuid::Uuid::new_v4().to_string(),
        path: "/original".into(),
        name: None,
        enabled: true,
    };
    srv.state
        .db
        .config()
        .upsert_root_folder(&folder)
        .await
        .unwrap();

    // Backup, then mutate (add a second folder the backup doesn't have).
    let created: serde_json::Value = client
        .post(srv.url("/api/v3/system/backup"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let backup_id = created["backupId"].as_str().unwrap().to_string();

    let folder2 = cellarr_core::RootFolder {
        id: uuid::Uuid::new_v4().to_string(),
        path: "/added".into(),
        name: None,
        enabled: true,
    };
    srv.state
        .db
        .config()
        .upsert_root_folder(&folder2)
        .await
        .unwrap();
    assert_eq!(
        srv.state
            .db
            .config()
            .list_root_folders()
            .await
            .unwrap()
            .len(),
        2
    );

    // Restore: a pre-restore safety backup is taken, the file swapped atomically.
    let restored: serde_json::Value = client
        .post(srv.url(&format!("/api/v3/system/backup/restore/{backup_id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(restored["restartRequired"], true);
    let safety_id = restored["safetyBackupId"].as_str().unwrap().to_string();
    assert!(!safety_id.is_empty());

    // The swapped-in file takes effect on restart: drop the live pool, reopen.
    srv.state.db.shutdown().await;
    let reopened = Database::open(srv.db_path.to_str().unwrap()).await.unwrap();
    let folders = reopened.config().list_root_folders().await.unwrap();
    assert_eq!(
        folders.len(),
        1,
        "restored to the single-folder backup state"
    );
    assert_eq!(folders[0].path, "/original");
    reopened.shutdown().await;
}

#[tokio::test]
async fn restore_upload_rejects_garbage_with_bad_request() {
    let srv = start().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(srv.url("/api/v3/system/backup/restore/upload"))
        .body(b"not a backup bundle".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "bad_request");
}

// --- log files -------------------------------------------------------------

#[tokio::test]
async fn log_file_list_and_tail_respects_limit() {
    let srv = start().await;
    let client = reqwest::Client::new();

    // Write a log file the daemon would have produced.
    let body = (1..=20)
        .map(|i| format!("entry {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(srv.log_dir.join("cellarr.log"), &body).unwrap();

    // List shows it.
    let list: serde_json::Value = client
        .get(srv.url("/api/v3/log/file"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["filename"] == "cellarr.log"));

    // Tail with a limit returns exactly the last N lines.
    let text = client
        .get(srv.url("/api/v3/log/file/cellarr.log?limit=3"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines, vec!["entry 18", "entry 19", "entry 20"]);
}

#[tokio::test]
async fn log_file_rejects_path_traversal() {
    let srv = start().await;
    let client = reqwest::Client::new();
    std::fs::write(srv.log_dir.join("cellarr.log"), "safe\n").unwrap();

    // A traversal attempt is a 404, never serving a file outside the logs dir.
    let resp = client
        .get(srv.url("/api/v3/log/file/..%2F..%2Fcellarr.sqlite"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

// --- health breadth --------------------------------------------------------

#[tokio::test]
async fn health_reports_missing_root_and_clients_and_no_recent_backup() {
    let srv = start().await;
    let client = reqwest::Client::new();

    let checks: serde_json::Value = client
        .get(srv.url("/api/v3/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = checks.as_array().unwrap();

    let has = |source: &str, ty: &str| arr.iter().any(|c| c["source"] == source && c["type"] == ty);
    // no-root-folder is an error; no-indexer / no-download-client are warnings.
    assert!(
        has("RootFolderCheck", "error"),
        "expected no-root-folder error"
    );
    assert!(has("IndexerCheck", "warning"));
    assert!(has("DownloadClientCheck", "warning"));
    // no-recent-backup warning (no backup taken yet).
    assert!(has("BackupCheck", "warning"), "expected no-recent-backup");
    // database-ok: there must be NO DatabaseCheck finding (the probe passed).
    assert!(!arr.iter().any(|c| c["source"] == "DatabaseCheck"));
    // Each record carries a wiki-ish type url.
    for c in arr {
        assert!(c["wikiUrl"].as_str().unwrap().contains("/docs/health#"));
    }
}

#[tokio::test]
async fn health_db_ok_clears_after_a_recent_backup_and_root_folder() {
    let srv = start().await;
    let client = reqwest::Client::new();

    // Give it a writable root folder + take a backup so two checks clear.
    let root = srv._dir.path().join("media");
    std::fs::create_dir_all(&root).unwrap();
    let folder = cellarr_core::RootFolder {
        id: uuid::Uuid::new_v4().to_string(),
        path: root.to_str().unwrap().into(),
        name: None,
        enabled: true,
    };
    srv.state
        .db
        .config()
        .upsert_root_folder(&folder)
        .await
        .unwrap();
    client
        .post(srv.url("/api/v3/system/backup"))
        .send()
        .await
        .unwrap();

    let checks: serde_json::Value = client
        .get(srv.url("/api/v3/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = checks.as_array().unwrap();
    assert!(!arr.iter().any(|c| c["source"] == "RootFolderCheck"));
    assert!(!arr.iter().any(|c| c["source"] == "BackupCheck"));
}
