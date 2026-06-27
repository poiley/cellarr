//! End-to-end tests for config-as-code reconciliation against a real database.
//!
//! These exercise the whole engine — load → interpolate → validate → plan →
//! apply — against an in-memory cellarr DB, asserting the behaviours the task
//! requires: idempotency, safe prune (config-managed only; UI-created survives),
//! cross-section creation in dependency order, export round-trip, and that a
//! malformed/invalid file fails loudly.

use cellarr_cli::managed::{self, loader, reconcile};
use cellarr_core::repo::ProfileRepository;
use cellarr_db::Database;

/// Open a fresh file-backed DB under a tempdir. We use a file (not the in-memory
/// DB) because reconciliation interleaves pool reads with writer-actor writes, and
/// the in-memory pool is pinned to a single connection the writer holds for life —
/// a read would deadlock. The tempdir is returned so it outlives the DB.
async fn temp_db() -> (Database, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cellarr.sqlite");
    let db = Database::open(path.to_str().unwrap())
        .await
        .expect("open db");
    (db, dir)
}

/// Load a config from inline YAML with an explicit env, panicking on error.
fn load(text: &str, env: &[(&str, &str)]) -> managed::ManagedConfig {
    let map: std::collections::HashMap<String, String> = env
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    loader::load_str(text, move |k| map.get(k).cloned()).expect("valid config")
}

const FULL: &str = r#"
apiVersion: cellarr/v1
tags:
  - name: anime
qualityDefinitions:
  - name: Bluray-1080p
    minSizePerMin: 5
customFormats:
  - name: x265
    score: -50
    conditions:
      - kind: codec
        codec: x265
qualityProfiles:
  - name: HD
    qualities: [WEBDL-1080p, Bluray-1080p]
    cutoff: Bluray-1080p
    customFormatScores:
      x265: -50
rootFolders:
  - name: movies
    path: /data/movies
libraries:
  - name: Movies
    mediaType: movie
    rootFolders: [movies]
    qualityProfile: HD
indexers:
  - name: nzbgeek
    kind: newznab
    protocol: usenet
    tags: [anime]
    settings:
      baseUrl: https://api.nzbgeek.info
      apiKey: ${NZBGEEK_KEY}
downloadClients:
  - name: qbit
    kind: qbittorrent
    protocol: torrent
    category: cellarr
    settings:
      host: localhost
      port: 8080
"#;

#[tokio::test]
async fn apply_creates_all_declared_entities() {
    let (db, _dir) = temp_db().await;
    let cfg = load(FULL, &[("NZBGEEK_KEY", "secret-key")]);

    let report = reconcile::apply(&db, &cfg).await.unwrap();
    let totals = report.totals();
    // Eight kinds, one item each, all created.
    assert_eq!(totals.created, 8, "{report:?}");
    assert_eq!(totals.pruned, 0);

    // Spot-check each kind landed in the DB.
    assert_eq!(db.tags().list().await.unwrap().len(), 1);
    assert_eq!(db.profiles().custom_formats().await.unwrap().len(), 1);
    assert_eq!(db.profiles().list_profiles().await.unwrap().len(), 1);
    assert_eq!(db.config().list_root_folders().await.unwrap().len(), 1);
    assert_eq!(db.config().list_libraries().await.unwrap().len(), 1);
    let indexers = db.config().list_indexers().await.unwrap();
    assert_eq!(indexers.len(), 1);
    // The secret was interpolated into the stored settings.
    assert_eq!(indexers[0].settings["apiKey"], "secret-key");
    // The indexer's tag scoping resolved the tag NAME to its integer id.
    let anime_id = db.tags().list().await.unwrap()[0].id;
    assert_eq!(indexers[0].tags, vec![anime_id]);
    assert_eq!(db.config().list_download_clients().await.unwrap().len(), 1);

    // The library resolved its profile + root folder by name.
    let lib = &db.config().list_libraries().await.unwrap()[0];
    let profile = &db.profiles().list_profiles().await.unwrap()[0];
    assert_eq!(lib.default_quality_profile, profile.id);
    assert_eq!(lib.root_folders.len(), 1);

    db.shutdown().await;
}

#[tokio::test]
async fn applying_twice_is_idempotent() {
    let (db, _dir) = temp_db().await;
    let cfg = load(FULL, &[("NZBGEEK_KEY", "secret-key")]);

    reconcile::apply(&db, &cfg).await.unwrap();
    // Second apply: the plan must be all-unchanged, zero writes.
    let report = reconcile::apply(&db, &cfg).await.unwrap();
    let totals = report.totals();
    assert_eq!(
        totals.created, 0,
        "second apply created something: {report:?}"
    );
    assert_eq!(
        totals.updated, 0,
        "second apply updated something: {report:?}"
    );
    assert_eq!(totals.pruned, 0);
    assert_eq!(totals.unchanged, 8);
    assert!(!report.has_changes());

    db.shutdown().await;
}

#[tokio::test]
async fn editing_an_item_plans_an_update() {
    let (db, _dir) = temp_db().await;
    reconcile::apply(&db, &load(FULL, &[("NZBGEEK_KEY", "k")]))
        .await
        .unwrap();

    // Change the download client's port — same name, different content.
    let edited = FULL.replace("port: 8080", "port: 9090");
    let report = reconcile::apply(&db, &load(&edited, &[("NZBGEEK_KEY", "k")]))
        .await
        .unwrap();
    let totals = report.totals();
    assert_eq!(totals.updated, 1, "{report:?}");
    assert_eq!(totals.created, 0);
    assert_eq!(totals.unchanged, 7);

    let dc = &db.config().list_download_clients().await.unwrap()[0];
    assert_eq!(dc.settings["port"], 9090);

    db.shutdown().await;
}

#[tokio::test]
async fn prune_removes_only_config_managed_entities() {
    let (db, _dir) = temp_db().await;

    // A UI-created indexer (NOT in the tracking ledger).
    let ui_indexer = cellarr_core::IndexerConfig {
        id: cellarr_core::IndexerId::new(),
        name: "ui-indexer".into(),
        kind: "torznab".into(),
        protocol: cellarr_core::Protocol::Torrent,
        enabled: true,
        priority: 0,
        criteria: cellarr_core::IndexerCriteria::default(),
        tags: vec![],
        settings: serde_json::json!({ "baseUrl": "https://ui" }),
    };
    db.config().upsert_indexer(&ui_indexer).await.unwrap();

    // A config that manages exactly one indexer.
    let cfg_with = r#"
apiVersion: cellarr/v1
indexers:
  - name: config-indexer
    kind: torznab
    protocol: torrent
    settings:
      baseUrl: https://config
"#;
    reconcile::apply(&db, &load(cfg_with, &[])).await.unwrap();
    assert_eq!(db.config().list_indexers().await.unwrap().len(), 2);

    // Now the config no longer declares any indexer (empty section = manage none).
    let cfg_empty = "apiVersion: cellarr/v1\nindexers: []\n";
    let report = reconcile::apply(&db, &load(cfg_empty, &[])).await.unwrap();
    assert_eq!(report.totals().pruned, 1, "{report:?}");

    // The config-managed indexer is gone; the UI-created one survives.
    let remaining = db.config().list_indexers().await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, "ui-indexer");

    db.shutdown().await;
}

#[tokio::test]
async fn absent_section_is_left_untouched() {
    let (db, _dir) = temp_db().await;

    // First apply manages an indexer.
    let with_ix = r#"
apiVersion: cellarr/v1
indexers:
  - name: ix
    kind: torznab
    protocol: torrent
    settings: { baseUrl: https://x }
"#;
    reconcile::apply(&db, &load(with_ix, &[])).await.unwrap();
    assert_eq!(db.config().list_indexers().await.unwrap().len(), 1);

    // A config that OMITS the indexers section entirely (vs an empty list) must
    // not touch the managed indexer.
    let no_ix = "apiVersion: cellarr/v1\nrootFolders: []\n";
    reconcile::apply(&db, &load(no_ix, &[])).await.unwrap();
    assert_eq!(
        db.config().list_indexers().await.unwrap().len(),
        1,
        "an omitted section must not prune its entities"
    );

    db.shutdown().await;
}

#[tokio::test]
async fn export_then_reimport_is_an_empty_plan() {
    let (db, _dir) = temp_db().await;
    reconcile::apply(&db, &load(FULL, &[("NZBGEEK_KEY", "secret")]))
        .await
        .unwrap();

    // Export the current state to YAML.
    let exported = managed::export::export(&db).await.unwrap();
    let yaml = managed::export::to_yaml(&exported).unwrap();

    // The export must redact the secret (no literal key in the committed file) and
    // emit a `${ENV}` placeholder in its place — the documented behaviour that lets
    // the file be committed safely.
    assert!(
        !yaml.contains("secret"),
        "exported YAML leaked a secret:\n{yaml}"
    );
    assert!(
        yaml.contains("${NZBGEEK_APIKEY}"),
        "export should emit an env placeholder for the secret:\n{yaml}"
    );

    // Re-import the exported YAML, supplying the original secret via the env var the
    // placeholder names. The round-trip is then exact: re-planning against the same
    // DB is empty (zero create/update/prune).
    let reimported = loader::load_str(&yaml, |k| {
        (k == "NZBGEEK_APIKEY").then(|| "secret".to_string())
    })
    .expect("re-imported export is valid");
    let report = reconcile::plan(&db, &reimported).await.unwrap();
    assert!(
        !report.has_changes(),
        "export -> re-import was not an empty plan: {}",
        managed::render_diff(&report)
    );

    db.shutdown().await;
}

#[tokio::test]
async fn plan_is_read_only() {
    let (db, _dir) = temp_db().await;
    let cfg = load(FULL, &[("NZBGEEK_KEY", "k")]);

    // A dry-run plan must not write anything.
    let report = reconcile::plan(&db, &cfg).await.unwrap();
    assert_eq!(report.totals().created, 8);
    assert!(db.config().list_indexers().await.unwrap().is_empty());
    assert!(db.tags().list().await.unwrap().is_empty());

    db.shutdown().await;
}

#[tokio::test]
async fn managed_config_ledger_crud_round_trips() {
    // Direct coverage of the tracking-ledger repo (and, by opening a fresh DB, that
    // migration 0017 applies from empty).
    use cellarr_db::ManagedEntity;
    let (db, _dir) = temp_db().await;
    let repo = db.managed_config();

    assert!(repo.list_all().await.unwrap().is_empty());

    let row = ManagedEntity {
        kind: "indexer".into(),
        name: "nzbgeek".into(),
        entity_id: "id-1".into(),
        content_hash: "h1".into(),
    };
    repo.upsert(&row).await.unwrap();
    assert_eq!(repo.list_kind("indexer").await.unwrap(), vec![row.clone()]);

    // Upsert with the same (kind,name) replaces the hash/id in place.
    let updated = ManagedEntity {
        content_hash: "h2".into(),
        ..row.clone()
    };
    repo.upsert(&updated).await.unwrap();
    assert_eq!(
        repo.list_kind("indexer").await.unwrap()[0].content_hash,
        "h2"
    );

    // A different kind is isolated.
    assert!(repo.list_kind("tag").await.unwrap().is_empty());

    // Delete reports removal and is idempotent.
    assert!(repo.delete("indexer", "nzbgeek").await.unwrap());
    assert!(!repo.delete("indexer", "nzbgeek").await.unwrap());
    assert!(repo.list_all().await.unwrap().is_empty());

    db.shutdown().await;
}

#[tokio::test]
async fn prune_removes_quality_definition_and_library_entities() {
    // Exercises the two delete methods added for prune (quality definition by name,
    // library by id) through a real reconcile.
    let (db, _dir) = temp_db().await;

    let with = r#"
apiVersion: cellarr/v1
qualityDefinitions:
  - name: Bluray-1080p
    minSizePerMin: 7
qualityProfiles:
  - name: HD
    qualities: [Bluray-1080p]
rootFolders:
  - name: movies
    path: /data/movies
libraries:
  - name: Movies
    mediaType: movie
    rootFolders: [movies]
    qualityProfile: HD
"#;
    reconcile::apply(&db, &load(with, &[])).await.unwrap();
    // The override row and the library exist.
    assert_eq!(
        db.profiles()
            .quality_definition_overrides()
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(db.config().list_libraries().await.unwrap().len(), 1);

    // Drop the quality-definition + library sections (declare them empty → prune).
    // The profile + root folder stay declared so the library's refs still validate
    // is irrelevant here since the library itself is being pruned.
    let without = r#"
apiVersion: cellarr/v1
qualityDefinitions: []
qualityProfiles:
  - name: HD
    qualities: [Bluray-1080p]
rootFolders:
  - name: movies
    path: /data/movies
libraries: []
"#;
    let report = reconcile::apply(&db, &load(without, &[])).await.unwrap();
    assert_eq!(report.totals().pruned, 2, "{report:?}");

    // The override was deleted (reverting to the code default) and the library gone.
    assert!(db
        .profiles()
        .quality_definition_overrides()
        .await
        .unwrap()
        .is_empty());
    assert!(db.config().list_libraries().await.unwrap().is_empty());

    db.shutdown().await;
}

#[tokio::test]
async fn malformed_config_fails_to_load() {
    // Unknown field => hard error (no half-applied config).
    let err = loader::load_str("apiVersion: cellarr/v1\nbogus: 1\n", |_| None).unwrap_err();
    assert!(err.to_string().contains("unknown field"), "{err}");
}

#[tokio::test]
async fn broken_cross_reference_fails_to_load() {
    let text = r#"
apiVersion: cellarr/v1
libraries:
  - name: L
    mediaType: movie
    rootFolders: [nope]
    qualityProfile: ghost
"#;
    let err = loader::load_str(text, |_| None).unwrap_err();
    assert!(
        err.to_string().contains("ghost") || err.to_string().contains("nope"),
        "{err}"
    );
}

#[tokio::test]
async fn missing_required_secret_fails_to_load() {
    let text = r#"
apiVersion: cellarr/v1
indexers:
  - name: ix
    kind: torznab
    protocol: torrent
    settings:
      apiKey: ${UNSET_SECRET}
"#;
    let err = loader::load_str(text, |_| None).unwrap_err();
    assert!(err.to_string().contains("UNSET_SECRET"), "{err}");
}
