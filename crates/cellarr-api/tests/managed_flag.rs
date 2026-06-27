//! The read-only `managed` flag on `/api/v3` (+ the native `/api/v1` library
//! endpoint).
//!
//! Every resource kind the config-as-code reconciler owns records a tracking-ledger
//! row keyed by `(kind, entity_id)`. The API derives an additive, read-only
//! `managed` boolean from that ledger so the UI can badge + lock config-managed
//! entities. These tests prove the flag is `true` for an entity with a ledger row
//! (config-managed) and `false` for a UI-created one (no ledger row), across a
//! representative span of kinds, and that the field is purely additive (no existing
//! field changes).

mod common;

use cellarr_db::ManagedEntity;
use serde_json::Value;

/// Mark an entity as config-managed by writing the tracking-ledger row the
/// reconciler would have written (kind + name + the entity's text id). The
/// content hash is irrelevant to the read-only flag, so any value works.
async fn mark_managed(state: &cellarr_api::AppState, kind: &str, name: &str, entity_id: &str) {
    state
        .db
        .managed_config()
        .upsert(&ManagedEntity {
            kind: kind.to_string(),
            name: name.to_string(),
            entity_id: entity_id.to_string(),
            content_hash: "test-hash".to_string(),
        })
        .await
        .expect("write ledger row");
}

#[tokio::test]
async fn indexer_managed_flag_reflects_the_ledger() {
    let srv = common::start_open().await;

    // One config-managed indexer (ledger row) and one UI-created (no row).
    let managed = common::seed_indexer(&srv.state, "config-indexer").await;
    let ui = common::seed_indexer(&srv.state, "ui-indexer").await;
    mark_managed(
        &srv.state,
        "indexer",
        "config-indexer",
        &managed.id.to_string(),
    )
    .await;

    let body: Vec<Value> = srv
        .client()
        .get(srv.url("/api/v3/indexer"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let by_name = |name: &str| -> Value {
        body.iter()
            .find(|v| v["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("indexer {name} missing from list: {body:?}"))
    };

    // The config-managed indexer is flagged; the UI-created one is not.
    assert_eq!(by_name("config-indexer")["managed"], Value::Bool(true));
    assert_eq!(by_name("ui-indexer")["managed"], Value::Bool(false));

    // The flag is purely additive — every existing field still present.
    let mgd = by_name("config-indexer");
    assert_eq!(mgd["name"], "config-indexer");
    assert!(mgd["fields"].is_array());
    assert_eq!(mgd["protocol"], "torrent");

    // Crucially, NOT a real managed id elsewhere: the UI indexer id is absent from
    // the ledger, so a future reconcile would never prune it (the flag is the UI's
    // signal for exactly that).
    assert_ne!(ui.id, managed.id);
}

#[tokio::test]
async fn download_client_managed_flag() {
    let srv = common::start_open().await;
    let managed = common::seed_download_client(&srv.state, "config-dc").await;
    let _ui = common::seed_download_client(&srv.state, "ui-dc").await;
    mark_managed(
        &srv.state,
        "download_client",
        "config-dc",
        &managed.id.to_string(),
    )
    .await;

    let body: Vec<Value> = srv
        .client()
        .get(srv.url("/api/v3/downloadclient"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let flag = |name: &str| body.iter().find(|v| v["name"] == name).unwrap()["managed"].clone();
    assert_eq!(flag("config-dc"), Value::Bool(true));
    assert_eq!(flag("ui-dc"), Value::Bool(false));
}

#[tokio::test]
async fn tag_managed_flag() {
    let srv = common::start_open().await;
    let managed_tag = srv.state.db.tags().create("config-tag").await.unwrap();
    let _ui_tag = srv.state.db.tags().create("ui-tag").await.unwrap();
    mark_managed(&srv.state, "tag", "config-tag", &managed_tag.id.to_string()).await;

    let body: Vec<Value> = srv
        .client()
        .get(srv.url("/api/v3/tag"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let flag = |label: &str| body.iter().find(|v| v["label"] == label).unwrap()["managed"].clone();
    assert_eq!(flag("config-tag"), Value::Bool(true));
    assert_eq!(flag("ui-tag"), Value::Bool(false));
}

#[tokio::test]
async fn quality_profile_managed_flag() {
    let srv = common::start_open().await;
    let managed_id = common::seed_profile(&srv.state, "config-profile").await;
    let _ui_id = common::seed_profile(&srv.state, "ui-profile").await;
    mark_managed(
        &srv.state,
        "quality_profile",
        "config-profile",
        &managed_id.to_string(),
    )
    .await;

    let body: Vec<Value> = srv
        .client()
        .get(srv.url("/api/v3/qualityprofile"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let flag = |name: &str| body.iter().find(|v| v["name"] == name).unwrap()["managed"].clone();
    assert_eq!(flag("config-profile"), Value::Bool(true));
    assert_eq!(flag("ui-profile"), Value::Bool(false));
}

#[tokio::test]
async fn library_managed_flag_on_native_api() {
    let srv = common::start_open().await;
    let managed_id =
        common::seed_library(&srv.state, cellarr_core::MediaType::Movie, "ConfigLib").await;
    let _ui_id = common::seed_library(&srv.state, cellarr_core::MediaType::Tv, "UiLib").await;
    mark_managed(&srv.state, "library", "ConfigLib", &managed_id.to_string()).await;

    let body: Vec<Value> = srv
        .client()
        .get(srv.url("/api/v1/libraries"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let flag = |name: &str| body.iter().find(|v| v["name"] == name).unwrap()["managed"].clone();
    assert_eq!(flag("ConfigLib"), Value::Bool(true));
    assert_eq!(flag("UiLib"), Value::Bool(false));

    // Additive: the typed Library fields still round-trip alongside `managed`.
    let lib = body.iter().find(|v| v["name"] == "ConfigLib").unwrap();
    assert_eq!(lib["media_type"], "movie");
    assert!(lib["root_folders"].is_array());
}
