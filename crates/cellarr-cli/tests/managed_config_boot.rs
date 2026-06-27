//! Boot integration for config-as-code: a managed-config path makes the daemon
//! reconcile its DB on boot (after migrations, before serving), and a
//! managed-config error fails boot loudly rather than serving stale/half-applied
//! config.
//!
//! Hermetic per `docs/16-local-dev-and-testing.md`: tempdir data dir, port 0.

use cellarr_cli::boot::Daemon;
use cellarr_cli::config::Config;
use cellarr_db::Database;

/// A managed config referencing `${var}` for its indexer api key. Parameterized
/// so each test uses a uniquely-named env var, avoiding cross-test env races
/// (cargo runs test fns in one process; a shared var name would interfere).
fn valid_with_secret_var(var: &str) -> String {
    format!(
        "apiVersion: cellarr/v1\n\
         rootFolders:\n  - name: movies\n    path: /data/movies\n\
         indexers:\n  - name: nzbgeek\n    kind: newznab\n    protocol: usenet\n    \
         settings:\n      baseUrl: https://api.nzbgeek.info\n      apiKey: ${{{var}}}\n"
    )
}

#[tokio::test]
async fn boot_reconciles_a_valid_managed_config() {
    let var = "CELLARR_TEST_BOOT_OK_KEY";
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("managed.yaml");
    std::fs::write(&cfg_path, valid_with_secret_var(var)).unwrap();

    // Provide the referenced secret in the process env (uniquely named).
    std::env::set_var(var, "boot-secret");

    let mut config = Config {
        data_dir: dir.path().to_path_buf(),
        ..Config::default()
    };
    config.api.port = 0;
    config.managed_config_path = Some(cfg_path);

    let db_path = config.database_path();
    let daemon = Daemon::boot(&config)
        .await
        .expect("boot reconciles cleanly");
    drop(daemon); // we only care that boot applied the config

    std::env::remove_var(var);

    // The declared entities were created during boot.
    let db = Database::open(db_path.to_str().unwrap()).await.unwrap();
    let indexers = db.config().list_indexers().await.unwrap();
    assert_eq!(indexers.len(), 1);
    assert_eq!(indexers[0].name, "nzbgeek");
    assert_eq!(indexers[0].settings["apiKey"], "boot-secret");
    assert_eq!(db.config().list_root_folders().await.unwrap().len(), 1);
    db.shutdown().await;
}

#[tokio::test]
async fn boot_fails_on_a_malformed_managed_config() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("managed.yaml");
    // Unknown field => the loader rejects it; boot must fail.
    std::fs::write(&cfg_path, "apiVersion: cellarr/v1\nbogusSection: []\n").unwrap();

    let mut config = Config {
        data_dir: dir.path().to_path_buf(),
        ..Config::default()
    };
    config.api.port = 0;
    config.managed_config_path = Some(cfg_path);

    let result = Daemon::boot(&config).await;
    assert!(
        result.is_err(),
        "boot must fail loudly on an invalid managed config"
    );
    let msg = format!("{:#}", result.err().unwrap());
    assert!(
        msg.contains("managed config") || msg.contains("unknown field"),
        "error should mention the managed config: {msg}"
    );
}

#[tokio::test]
async fn boot_fails_on_a_missing_required_secret() {
    let var = "CELLARR_TEST_BOOT_MISSING_KEY";
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("managed.yaml");
    std::fs::write(&cfg_path, valid_with_secret_var(var)).unwrap();

    // Deliberately do NOT set the (uniquely-named) var: the reference is
    // unresolved, which must fail boot.
    std::env::remove_var(var);

    let mut config = Config {
        data_dir: dir.path().to_path_buf(),
        ..Config::default()
    };
    config.api.port = 0;
    config.managed_config_path = Some(cfg_path);

    let result = Daemon::boot(&config).await;
    assert!(result.is_err(), "missing required secret must fail boot");
    assert!(format!("{:#}", result.err().unwrap()).contains(var));
}

#[tokio::test]
async fn no_managed_config_path_is_unchanged_behaviour() {
    // Zero-config (no managed path) still boots fine and creates nothing managed.
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config {
        data_dir: dir.path().to_path_buf(),
        ..Config::default()
    };
    config.api.port = 0;
    assert!(config.managed_config_path.is_none());

    let daemon = Daemon::boot(&config).await.expect("zero-config boot");
    let db_path = config.database_path();
    drop(daemon);

    let db = Database::open(db_path.to_str().unwrap()).await.unwrap();
    assert!(db.config().list_indexers().await.unwrap().is_empty());
    assert!(db.managed_config().list_all().await.unwrap().is_empty());
    db.shutdown().await;
}
