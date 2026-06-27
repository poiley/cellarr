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
releaseProfiles:
  - name: no-x265
    tags: [anime]
    ignored: [x265]
    preferred:
      - term: PROPER
        score: 5
delayProfiles:
  - name: default
    usenetDelay: 30
    torrentDelay: 60
    preferredProtocol: usenet
importLists:
  - name: my-trakt
    kind: trakt
    mediaType: movie
    qualityProfile: HD
    settings:
      list: my-list
      apiKey: ${TRAKT_ID}
notifications:
  - name: discord
    kind: discord
    tags: [anime]
    onEvents: [grab, import]
    settings:
      webhookUrl: ${DISCORD_WEBHOOK}
remotePathMappings:
  - name: dl
    host: qbit.local
    remotePath: /downloads
    localPath: /data/downloads
naming:
  movieFileFormat: "{Movie Title}/movie.{Extension}"
mediaManagement:
  recycleBinPath: /recycle
  writeNfo: false
auth:
  method: forms
  username: admin
  passwordHash: ${AUTH_HASH}
"#;

/// The full env the [`FULL`] config references (all `${ENV}` secrets).
const FULL_ENV: &[(&str, &str)] = &[
    ("NZBGEEK_KEY", "secret-key"),
    ("TRAKT_ID", "trakt-client-id"),
    ("DISCORD_WEBHOOK", "https://discord/webhook"),
    ("AUTH_HASH", "$argon2id$v=19$m=4096$abc$def"),
];

#[tokio::test]
async fn apply_creates_all_declared_entities() {
    let (db, _dir) = temp_db().await;
    let cfg = load(FULL, FULL_ENV);

    let report = reconcile::apply(&db, &cfg).await.unwrap();
    let totals = report.totals();
    // 13 name-keyed kinds (one item each) + 3 singletons (naming, media-management,
    // auth), all created on first apply.
    assert_eq!(totals.created, 16, "{report:?}");
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

    // --- the operational-surface kinds (Pack 2) landed too --------------------
    let release_profiles = db.profiles().list_release_profiles().await.unwrap();
    assert_eq!(release_profiles.len(), 1);
    // The release profile's tag scoping resolved the tag NAME to its integer id.
    assert_eq!(release_profiles[0].tags, vec![anime_id]);
    assert_eq!(release_profiles[0].ignored, vec!["x265".to_string()]);

    let delay_profiles = db.profiles().list_delay_profiles().await.unwrap();
    assert_eq!(delay_profiles.len(), 1);
    assert_eq!(delay_profiles[0].usenet_delay, 30);

    use cellarr_core::importlist::ImportListRepository;
    let import_lists = db.import_lists().list().await.unwrap();
    assert_eq!(import_lists.len(), 1);
    // The import list resolved its quality-profile reference by name → id, and the
    // ${ENV} secret in its settings was interpolated.
    assert_eq!(
        import_lists[0].quality_profile_id.as_deref(),
        Some(profile.id.to_string().as_str())
    );
    assert_eq!(import_lists[0].settings["apiKey"], "trakt-client-id");

    let notifications = db.config().list_notifications().await.unwrap();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].tags, vec![anime_id]);
    assert_eq!(
        notifications[0].settings["webhookUrl"],
        "https://discord/webhook"
    );

    let mappings = db.config().list_remote_path_mappings().await.unwrap();
    assert_eq!(mappings.len(), 1);
    assert_eq!(mappings[0].remote_path, "/downloads");

    // The singletons were set as whole documents.
    let mm = db.config().get_media_management().await.unwrap();
    assert_eq!(
        mm.naming.movie_file_format,
        "{Movie Title}/movie.{Extension}"
    );
    assert_eq!(mm.recycle_bin_path.as_deref(), Some("/recycle"));
    assert!(!mm.write_nfo);

    let auth = db.auth().get_config().await.unwrap();
    assert_eq!(auth.method, cellarr_core::AuthMethod::Forms);
    assert_eq!(auth.username.as_deref(), Some("admin"));
    assert_eq!(
        auth.password_hash.as_deref(),
        Some("$argon2id$v=19$m=4096$abc$def")
    );

    db.shutdown().await;
}

#[tokio::test]
async fn applying_twice_is_idempotent() {
    let (db, _dir) = temp_db().await;
    let cfg = load(FULL, FULL_ENV);

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
    assert_eq!(totals.unchanged, 16);
    assert!(!report.has_changes());

    db.shutdown().await;
}

#[tokio::test]
async fn editing_an_item_plans_an_update() {
    let (db, _dir) = temp_db().await;
    reconcile::apply(&db, &load(FULL, FULL_ENV)).await.unwrap();

    // Change the download client's port — same name, different content.
    let edited = FULL.replace("port: 8080", "port: 9090");
    let report = reconcile::apply(&db, &load(&edited, FULL_ENV))
        .await
        .unwrap();
    let totals = report.totals();
    assert_eq!(totals.updated, 1, "{report:?}");
    assert_eq!(totals.created, 0);
    assert_eq!(totals.unchanged, 15);

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
    reconcile::apply(&db, &load(FULL, FULL_ENV)).await.unwrap();

    // Export the current state to YAML.
    let exported = managed::export::export(&db).await.unwrap();
    let yaml = managed::export::to_yaml(&exported).unwrap();

    // The export must redact every secret (indexer key, notification webhook, the
    // auth password hash) to a `${ENV}` placeholder — never a literal — so the file
    // is safe to commit.
    for leaked in [
        "secret-key",
        "https://discord/webhook",
        "$argon2id",
        "trakt-client-id",
    ] {
        assert!(
            !yaml.contains(leaked),
            "exported YAML leaked a secret `{leaked}`:\n{yaml}"
        );
    }
    assert!(
        yaml.contains("${NZBGEEK_APIKEY}"),
        "export should emit an env placeholder for the indexer key:\n{yaml}"
    );
    assert!(
        yaml.contains("${AUTH_PASSWORD_HASH}"),
        "export should emit an env placeholder for the auth password hash:\n{yaml}"
    );

    // Re-import the exported YAML, supplying every redacted secret via the env var
    // its placeholder names. The round-trip is then exact: re-planning against the
    // same DB is empty (zero create/update/prune) — over the FULL set, singletons
    // included.
    let env: std::collections::HashMap<&str, &str> = [
        ("NZBGEEK_APIKEY", "secret-key"),
        ("DISCORD_WEBHOOKURL", "https://discord/webhook"),
        ("MY_TRAKT_APIKEY", "trakt-client-id"),
        ("AUTH_PASSWORD_HASH", "$argon2id$v=19$m=4096$abc$def"),
    ]
    .into_iter()
    .collect();
    let reimported = loader::load_str(&yaml, |k| env.get(k).map(|s| (*s).to_string()))
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
    let cfg = load(FULL, FULL_ENV);

    // A dry-run plan must not write anything.
    let report = reconcile::plan(&db, &cfg).await.unwrap();
    assert_eq!(report.totals().created, 16);
    assert!(db.config().list_indexers().await.unwrap().is_empty());
    assert!(db.tags().list().await.unwrap().is_empty());
    // Singletons were not written by the dry run either (still defaults).
    assert!(db.config().list_notifications().await.unwrap().is_empty());
    assert_eq!(
        db.auth().get_config().await.unwrap().method,
        cellarr_core::AuthMethod::None
    );

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
async fn prune_removes_only_config_managed_notifications() {
    // The safe-prune guarantee holds for the new name-keyed kinds too: a UI-created
    // notification (no ledger row) survives when config stops declaring its own.
    let (db, _dir) = temp_db().await;

    let ui = cellarr_core::NotificationConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name: "ui-notify".into(),
        kind: "webhook".into(),
        enabled: true,
        on_events: vec![],
        tags: vec![],
        settings: serde_json::json!({ "webhookUrl": "https://ui" }),
    };
    db.config().upsert_notification(&ui).await.unwrap();

    let with = r#"
apiVersion: cellarr/v1
notifications:
  - name: config-notify
    kind: discord
    settings: { webhookUrl: https://config }
"#;
    reconcile::apply(&db, &load(with, &[])).await.unwrap();
    assert_eq!(db.config().list_notifications().await.unwrap().len(), 2);

    // Stop declaring any notification (empty section = manage none) → prune the
    // config one only.
    let empty = "apiVersion: cellarr/v1\nnotifications: []\n";
    let report = reconcile::apply(&db, &load(empty, &[])).await.unwrap();
    assert_eq!(report.totals().pruned, 1, "{report:?}");
    let remaining = db.config().list_notifications().await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, "ui-notify");

    db.shutdown().await;
}

#[tokio::test]
async fn delay_and_remote_path_mapping_round_trip_by_config_name() {
    // The two name-less core models (delay profile, remote-path mapping) are keyed
    // by a config name in the ledger; create → idempotent re-apply → edit → prune.
    let (db, _dir) = temp_db().await;

    let cfg = r#"
apiVersion: cellarr/v1
delayProfiles:
  - name: anime-delay
    usenetDelay: 15
    tags: [anime]
remotePathMappings:
  - name: dl
    remotePath: /downloads
    localPath: /data/downloads
"#;
    let report = reconcile::apply(&db, &load(cfg, &[])).await.unwrap();
    assert_eq!(report.totals().created, 2, "{report:?}");
    assert_eq!(db.profiles().list_delay_profiles().await.unwrap().len(), 1);
    assert_eq!(
        db.config().list_remote_path_mappings().await.unwrap().len(),
        1
    );

    // Idempotent.
    let again = reconcile::apply(&db, &load(cfg, &[])).await.unwrap();
    assert!(!again.has_changes(), "{again:?}");

    // Edit the delay window → an in-place UPDATE (same config name → same id).
    let edited = cfg.replace("usenetDelay: 15", "usenetDelay: 45");
    let dp_before = db.profiles().list_delay_profiles().await.unwrap()[0].id;
    let upd = reconcile::apply(&db, &load(&edited, &[])).await.unwrap();
    assert_eq!(upd.totals().updated, 1, "{upd:?}");
    let dps = db.profiles().list_delay_profiles().await.unwrap();
    assert_eq!(dps[0].id, dp_before, "update must preserve the id");
    assert_eq!(dps[0].usenet_delay, 45);

    // Prune both by declaring them empty.
    let pruned = reconcile::apply(
        &db,
        &load(
            "apiVersion: cellarr/v1\ndelayProfiles: []\nremotePathMappings: []\n",
            &[],
        ),
    )
    .await
    .unwrap();
    assert_eq!(pruned.totals().pruned, 2, "{pruned:?}");
    assert!(db
        .profiles()
        .list_delay_profiles()
        .await
        .unwrap()
        .is_empty());
    assert!(db
        .config()
        .list_remote_path_mappings()
        .await
        .unwrap()
        .is_empty());

    db.shutdown().await;
}

#[tokio::test]
async fn singletons_update_and_are_idempotent() {
    // Naming / media-management / auth are whole-document singletons: first apply
    // creates, an unchanged re-apply is a no-op, an edit updates, and omitting the
    // section leaves the live document untouched.
    let (db, _dir) = temp_db().await;

    let cfg = r#"
apiVersion: cellarr/v1
naming:
  movieFileFormat: "{Movie Title}.{Extension}"
mediaManagement:
  recycleBinPath: /recycle
auth:
  method: basic
  username: admin
  passwordHash: hash-1
"#;
    let report = reconcile::apply(&db, &load(cfg, &[])).await.unwrap();
    assert_eq!(report.totals().created, 3, "{report:?}");

    // Idempotent: an identical re-apply changes nothing.
    let again = reconcile::apply(&db, &load(cfg, &[])).await.unwrap();
    assert!(!again.has_changes(), "{again:?}");

    // Editing one singleton (the recycle bin) is a single UPDATE; the others stay
    // unchanged.
    let edited = cfg.replace("/recycle", "/trash");
    let upd = reconcile::apply(&db, &load(&edited, &[])).await.unwrap();
    assert_eq!(upd.totals().updated, 1, "{upd:?}");
    assert_eq!(upd.totals().unchanged, 2, "{upd:?}");
    assert_eq!(
        db.config()
            .get_media_management()
            .await
            .unwrap()
            .recycle_bin_path
            .as_deref(),
        Some("/trash")
    );
    // The independently-declared naming singleton was preserved (not clobbered by
    // the media-management write, and vice-versa).
    assert_eq!(
        db.config()
            .get_media_management()
            .await
            .unwrap()
            .naming
            .movie_file_format,
        "{Movie Title}.{Extension}"
    );

    // Omitting the auth section entirely leaves the live auth config untouched.
    let no_auth = "apiVersion: cellarr/v1\nmediaManagement:\n  recycleBinPath: /trash\n";
    reconcile::apply(&db, &load(no_auth, &[])).await.unwrap();
    assert_eq!(
        db.auth().get_config().await.unwrap().method,
        cellarr_core::AuthMethod::Basic
    );

    db.shutdown().await;
}

#[tokio::test]
async fn auth_enforcing_without_credential_fails_validation() {
    // The lock-out guard: selecting forms/basic with no credential is rejected at
    // load time (it would lock the operator out).
    let text = r#"
apiVersion: cellarr/v1
auth:
  method: forms
"#;
    let err = loader::load_str(text, |_| None).unwrap_err();
    assert!(
        err.to_string().contains("lock the operator out") || err.to_string().contains("credential"),
        "{err}"
    );
}

#[tokio::test]
async fn import_list_missing_profile_fails_validation() {
    let text = r#"
apiVersion: cellarr/v1
importLists:
  - name: l
    kind: trakt
    mediaType: movie
    qualityProfile: ghost
    settings: {}
"#;
    let err = loader::load_str(text, |_| None).unwrap_err();
    assert!(err.to_string().contains("ghost"), "{err}");
}

#[tokio::test]
async fn notification_scoped_to_undeclared_tag_fails_validation() {
    let text = r#"
apiVersion: cellarr/v1
notifications:
  - name: n
    kind: discord
    tags: [missing]
    settings: {}
"#;
    let err = loader::load_str(text, |_| None).unwrap_err();
    assert!(err.to_string().contains("missing"), "{err}");
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
