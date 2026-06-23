//! Boot smoke test: a zero-config daemon boots, serves health, shuts down
//! gracefully, and leaves a consistent database.
//!
//! Pins the `docs/specs/cellarr-cli.md` obligations:
//! - boots from empty config (defaults only) into a working daemon,
//! - serves the system-status endpoint with 200,
//! - graceful shutdown drains the writer-actor / closes the pool, leaving a DB
//!   that reopens and answers a sanity query.
//!
//! No fixed ports (API binds `127.0.0.1:0`, the OS picks) and no fixed paths
//! (tempdir data dir), per `docs/16-local-dev-and-testing.md`.

use cellarr_cli::boot::Daemon;
use cellarr_cli::config::Config;
use cellarr_db::Database;

#[tokio::test]
async fn boots_serves_health_and_shuts_down_clean() {
    // Empty config = built-in defaults, except: a tempdir data dir and port 0 so
    // the test is hermetic and parallel-safe.
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = Config {
        data_dir: dir.path().to_path_buf(),
        ..Config::default()
    };
    config.api.port = 0;

    let db_path = config.database_path();

    // Boot: opens the DB (runs migrations), builds registries, binds the listener.
    let daemon = Daemon::boot(&config).await.expect("daemon boots");
    let addr = daemon.addr();
    assert_ne!(addr.port(), 0, "OS assigned a real port");

    // Serve until we fire the shutdown signal.
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let serve = tokio::spawn(async move {
        daemon
            .serve_until(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });

    // Health endpoint responds 200 with the expected app identity.
    let url = format!("http://{addr}/api/v1/system/status");
    let resp = wait_for_ok(&url).await;
    assert_eq!(resp.status(), 200, "system/status is healthy");
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["app_name"], "cellarr", "status reports the app name");
    assert_eq!(
        body["auth_enabled"], false,
        "zero-config first run has auth disabled"
    );

    // Trigger graceful shutdown and wait for the serve task to finish cleanly.
    shutdown_tx.send(()).expect("send shutdown");
    let result = serve.await.expect("serve task joins");
    result.expect("clean shutdown");

    // The DB is left consistent: reopen it (this also re-runs migrations, which
    // would fail on a torn schema) and run a sanity query that exercises a repo.
    let db = Database::open(db_path.to_str().expect("utf-8 path"))
        .await
        .expect("reopen DB after shutdown");
    let libraries = db
        .config()
        .list_libraries()
        .await
        .expect("sanity query succeeds on the reopened DB");
    assert!(
        libraries.is_empty(),
        "a fresh zero-config DB has no libraries yet"
    );
    db.shutdown().await;
}

/// Poll the URL until the just-spawned server is accepting (a freshly bound
/// listener may not have begun serving the instant the test races to it). Bounded
/// so a real failure surfaces as a test failure, not a hang.
async fn wait_for_ok(url: &str) -> reqwest::Response {
    let client = reqwest::Client::new();
    for _ in 0..50 {
        if let Ok(resp) = client.get(url).send().await {
            return resp;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("server did not become reachable at {url}");
}
