//! Phase A contract tests for the `/api/v3` drop-in surface.
//!
//! These assert cellarr is a real Sonarr + Radarr drop-in: the bug-B1 404-JSON
//! fix, the `X-Application-Version` header, both auth modes, the two faces
//! (`/sonarr/api/v3` and `/radarr/api/v3`), and that the new endpoints' JSON
//! shapes match the responses captured from live Sonarr 4.0.17 / Radarr 6.2.1
//! (the fixtures in `tests/fixtures/`).

mod common;

use common::{seed_indexer, seed_library, start_authed, start_open, TEST_API_KEY};
use serde_json::Value;
use std::collections::BTreeSet;

/// Load a captured fixture and return its parsed JSON.
fn fixture(rel: &str) -> Value {
    let path = format!("{}/tests/fixtures/{rel}", env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

/// The set of object keys at the top level of a JSON object.
fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// Top-level keys present in `want` but absent from `got`.
fn missing_keys(want: &Value, got: &Value) -> Vec<String> {
    let (w, g) = (keys(want), keys(got));
    w.difference(&g).cloned().collect()
}

// --- B1: unknown /api/v3/* returns 404 JSON, not SPA HTML -------------------

#[tokio::test]
async fn unknown_api_path_returns_404_json_not_html() {
    let server = start_open().await;
    for base in ["/api/v3", "/sonarr/api/v3", "/radarr/api/v3"] {
        let resp = server
            .client()
            .get(server.url(&format!("{base}/does-not-exist")))
            .send()
            .await
            .expect("request");
        assert_eq!(resp.status(), 404, "{base} unknown path must be 404");
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(
            ct.contains("application/json"),
            "{base} unknown path must be JSON, got {ct}"
        );
        let body: Value = resp.json().await.expect("json");
        assert!(body.get("code").is_some(), "404 body must carry a code");
    }
}

#[tokio::test]
async fn non_api_path_still_serves_spa() {
    let server = start_open().await;
    let resp = server
        .client()
        .get(server.url("/library"))
        .send()
        .await
        .expect("request");
    // The asset fallback still serves the SPA for non-API paths.
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(ct.contains("text/html"), "non-API path should be HTML");
}

// --- X-Application-Version header -------------------------------------------

#[tokio::test]
async fn version_header_present_per_face() {
    let server = start_open().await;
    seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;

    for (base, expect_prefix) in [
        ("/sonarr/api/v3", "4."),
        ("/radarr/api/v3", "5."),
        ("/api/v3", ""),
    ] {
        let resp = server
            .client()
            .get(server.url(&format!("{base}/system/status")))
            .send()
            .await
            .expect("request");
        let ver = resp
            .headers()
            .get("X-Application-Version")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(!ver.is_empty(), "{base} missing X-Application-Version");
        assert!(
            ver.starts_with(expect_prefix),
            "{base} version {ver} should start {expect_prefix}"
        );
    }
}

// --- both auth modes --------------------------------------------------------

#[tokio::test]
async fn both_auth_modes_accepted_when_key_set() {
    let server = start_authed().await;
    seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;

    // Header form.
    let r1 = server
        .client()
        .post(server.url("/api/v3/tag"))
        .header("X-Api-Key", TEST_API_KEY)
        .json(&serde_json::json!({ "label": "via-header" }))
        .send()
        .await
        .expect("request");
    assert_eq!(r1.status(), 200, "X-Api-Key header should authorize");

    // Query form.
    let r2 = server
        .client()
        .post(server.url(&format!("/api/v3/tag?apikey={TEST_API_KEY}")))
        .json(&serde_json::json!({ "label": "via-query" }))
        .send()
        .await
        .expect("request");
    assert_eq!(r2.status(), 200, "?apikey= should authorize");

    // No key → 401.
    let r3 = server
        .client()
        .post(server.url("/api/v3/tag"))
        .json(&serde_json::json!({ "label": "nope" }))
        .send()
        .await
        .expect("request");
    assert_eq!(r3.status(), 401);
}

// --- system/status full field set vs fixtures ------------------------------

#[tokio::test]
async fn status_matches_sonarr_field_set() {
    let server = start_open().await;
    seed_library(&server.state, cellarr_core::MediaType::Tv, "Shows").await;
    let body: Value = server
        .client()
        .get(server.url("/sonarr/api/v3/system/status"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(body["appName"], "Sonarr");
    assert!(body["version"].as_str().unwrap().starts_with("4."));
    let missing = missing_keys(&fixture("sonarr/system-status.json"), &body);
    assert!(
        missing.is_empty(),
        "sonarr status missing fields vs fixture: {missing:?}"
    );
}

#[tokio::test]
async fn status_matches_radarr_field_set() {
    let server = start_open().await;
    seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;
    let body: Value = server
        .client()
        .get(server.url("/radarr/api/v3/system/status"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(body["appName"], "Radarr");
    assert!(body["version"].as_str().unwrap().starts_with("5."));
    let missing = missing_keys(&fixture("radarr/system-status.json"), &body);
    assert!(missing.is_empty(), "radarr status missing: {missing:?}");
}

// --- qualityprofile: formatItems + minUpgradeFormatScore -------------------

#[tokio::test]
async fn qualityprofile_carries_format_items_and_scores() {
    let server = start_open().await;
    seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;
    // A custom format exists → it should appear in formatItems.
    let cf = cellarr_core::CustomFormat {
        id: cellarr_core::CustomFormatId::new(),
        name: "HD Bluray".into(),
        conditions: vec![],
        score: 100,
    };
    server
        .state
        .db
        .profiles()
        .upsert_custom_format(&cf)
        .await
        .unwrap();

    let body: Value = server
        .client()
        .get(server.url("/radarr/api/v3/qualityprofile"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    let p = &body.as_array().unwrap()[0];
    // Fields Recyclarr/Configarr need for CF-score sync.
    assert!(p.get("formatItems").is_some(), "missing formatItems");
    assert!(p.get("minUpgradeFormatScore").is_some());
    let fi = p["formatItems"].as_array().unwrap();
    assert_eq!(fi.len(), 1, "formatItems should list the custom format");
    assert_eq!(fi[0]["score"], 100);
    assert!(fi[0].get("format").is_some());
    // Radarr profiles carry language; Sonarr's do not — match the fixtures.
    let want = fixture("radarr/qualityprofile.json").as_array().unwrap()[0].clone();
    let missing = missing_keys(&want, p);
    assert!(missing.is_empty(), "radarr qp missing fields: {missing:?}");
}

#[tokio::test]
async fn qualityprofile_schema_present_per_face() {
    let server = start_open().await;
    for base in ["/sonarr/api/v3", "/radarr/api/v3"] {
        let body: Value = server
            .client()
            .get(server.url(&format!("{base}/qualityprofile/schema")))
            .send()
            .await
            .expect("request")
            .json()
            .await
            .expect("json");
        assert!(body.get("items").is_some(), "{base} schema missing items");
        assert!(body.get("formatItems").is_some());
        assert!(body["items"].as_array().unwrap().len() > 1);
    }
}

// --- customformat CRUD + schema round-trip ---------------------------------

#[tokio::test]
async fn customformat_round_trips_specifications() {
    let server = start_authed().await;

    // Schema first (Recyclarr validates against it).
    let schema: Value = server
        .client()
        .get(server.url("/api/v3/customformat/schema"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    let impls: BTreeSet<String> = schema
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["implementation"].as_str().unwrap().to_string())
        .collect();
    assert!(impls.contains("ReleaseTitleSpecification"));

    // Create with a Recyclarr-shaped body.
    let cf_body = serde_json::json!({
        "name": "x264",
        "includeCustomFormatWhenRenaming": false,
        "specifications": [{
            "name": "x264",
            "implementation": "ReleaseTitleSpecification",
            "negate": false,
            "required": false,
            "fields": [{ "name": "value", "value": "(x|h)\\.?264" }]
        }]
    });
    let created: Value = server
        .client()
        .post(server.url("/api/v3/customformat"))
        .header("X-Api-Key", TEST_API_KEY)
        .json(&cf_body)
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(created["name"], "x264");
    assert!(created.get("id").is_some());
    let spec = &created["specifications"][0];
    assert_eq!(spec["implementation"], "ReleaseTitleSpecification");
    assert_eq!(spec["fields"][0]["value"], "(x|h)\\.?264");

    // It now shows up in the list with the same spec.
    let list: Value = server
        .client()
        .get(server.url("/api/v3/customformat"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(list.as_array().unwrap().len(), 1);
    let missing = missing_keys(&fixture("sonarr/customformat-created.json"), &created);
    assert!(
        missing.is_empty(),
        "cf missing fields vs fixture: {missing:?}"
    );
}

// --- indexer CRUD + schema + test + forceSave ------------------------------

#[tokio::test]
async fn indexer_schema_has_torznab_and_newznab() {
    let server = start_open().await;
    let body: Value = server
        .client()
        .get(server.url("/api/v3/indexer/schema"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    let impls: BTreeSet<String> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["implementation"].as_str().unwrap().to_string())
        .collect();
    assert!(impls.contains("Torznab"), "schema missing Torznab");
    assert!(impls.contains("Newznab"), "schema missing Newznab");
    // Schema entries carry the fields Prowlarr round-trips.
    let torznab = body
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["implementation"] == "Torznab")
        .unwrap();
    let field_names: BTreeSet<String> = torznab["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_string())
        .collect();
    assert!(field_names.contains("baseUrl"));
    assert!(field_names.contains("apiKey"));
}

#[tokio::test]
async fn indexer_push_round_trips_and_force_save_honored() {
    let server = start_authed().await;
    let ind = serde_json::json!({
        "name": "Prowlarr Torznab",
        "implementation": "Torznab",
        "protocol": "torrent",
        "priority": 25,
        "enableRss": true,
        "fields": [
            { "name": "baseUrl", "value": "http://prowlarr.invalid" },
            { "name": "apiKey", "value": "abc" },
            { "name": "categories", "value": [5030, 5040] }
        ]
    });
    let created: Value = server
        .client()
        .post(server.url("/api/v3/indexer?forceSave=true"))
        .header("X-Api-Key", TEST_API_KEY)
        .json(&ind)
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(created["name"], "Prowlarr Torznab");
    assert_eq!(created["implementation"], "Torznab");
    assert!(created.get("id").is_some());
    // The pushed fields round-trip back.
    let names: BTreeSet<String> = created["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains("baseUrl"));
    assert!(names.contains("apiKey"));

    // It appears in the indexer list.
    let list: Value = server
        .client()
        .get(server.url("/api/v3/indexer"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(list.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn indexer_test_validates_body() {
    let server = start_authed().await;
    // Valid body → isValid true.
    let ok: Value = server
        .client()
        .post(server.url("/api/v3/indexer/test"))
        .header("X-Api-Key", TEST_API_KEY)
        .json(&serde_json::json!({
            "name": "t", "implementation": "Torznab",
            "fields": [{ "name": "baseUrl", "value": "http://x" }]
        }))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(ok["isValid"], true);

    // Missing baseUrl → 400.
    let bad = server
        .client()
        .post(server.url("/api/v3/indexer/test"))
        .header("X-Api-Key", TEST_API_KEY)
        .json(&serde_json::json!({ "name": "t", "implementation": "Torznab", "fields": [] }))
        .send()
        .await
        .expect("request");
    assert_eq!(bad.status(), 400);
}

// --- rootfolder / tag / health / qualitydefinition / wanted / GET command --

#[tokio::test]
async fn rootfolder_matches_fixture_shape() {
    let server = start_open().await;
    seed_library(&server.state, cellarr_core::MediaType::Tv, "Shows").await;
    let body: Value = server
        .client()
        .get(server.url("/sonarr/api/v3/rootfolder"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    let arr = body.as_array().unwrap();
    assert!(!arr.is_empty(), "seeded library should yield a root folder");
    let want: BTreeSet<String> = ["accessible", "freeSpace", "id", "path", "unmappedFolders"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let got = keys(&arr[0]);
    let missing: Vec<_> = want.difference(&got).collect();
    assert!(missing.is_empty(), "rootfolder missing: {missing:?}");
}

#[tokio::test]
async fn tag_crud_full_lifecycle() {
    let server = start_authed().await;
    let created: Value = server
        .client()
        .post(server.url("/api/v3/tag"))
        .header("X-Api-Key", TEST_API_KEY)
        .json(&serde_json::json!({ "label": "anime" }))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(created["label"], "anime");
    let id = created["id"].as_u64().unwrap();

    // List shows it.
    let list: Value = server
        .client()
        .get(server.url("/api/v3/tag"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Update.
    let updated: Value = server
        .client()
        .put(server.url(&format!("/api/v3/tag/{id}")))
        .header("X-Api-Key", TEST_API_KEY)
        .json(&serde_json::json!({ "label": "anime-renamed" }))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(updated["label"], "anime-renamed");

    // Delete.
    let del = server
        .client()
        .delete(server.url(&format!("/api/v3/tag/{id}")))
        .header("X-Api-Key", TEST_API_KEY)
        .send()
        .await
        .expect("request");
    assert_eq!(del.status(), 200);
}

#[tokio::test]
async fn health_reports_missing_config_as_v3_records() {
    let server = start_open().await;
    // Fresh: no indexers, no clients, no root folders → all three warnings.
    let body: Value = server
        .client()
        .get(server.url("/api/v3/health"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    let arr = body.as_array().unwrap();
    assert!(!arr.is_empty());
    let want = fixture("sonarr/health.json").as_array().unwrap()[0].clone();
    let missing = missing_keys(&want, &arr[0]);
    assert!(missing.is_empty(), "health record missing: {missing:?}");
}

#[tokio::test]
async fn qualitydefinition_present() {
    let server = start_open().await;
    let body: Value = server
        .client()
        .get(server.url("/api/v3/qualitydefinition"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    let arr = body.as_array().unwrap();
    assert!(!arr.is_empty());
    let want = fixture("sonarr/qualitydefinition.json").as_array().unwrap()[0].clone();
    let missing = missing_keys(&want, &arr[0]);
    assert!(missing.is_empty(), "qualitydefinition missing: {missing:?}");
}

#[tokio::test]
async fn wanted_missing_has_full_paging_envelope() {
    let server = start_open().await;
    let body: Value = server
        .client()
        .get(server.url("/sonarr/api/v3/wanted/missing"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    for k in [
        "page",
        "pageSize",
        "sortKey",
        "sortDirection",
        "totalRecords",
        "records",
    ] {
        assert!(body.get(k).is_some(), "wanted/missing missing {k}");
    }
}

#[tokio::test]
async fn queue_history_have_full_paging_envelope() {
    let server = start_open().await;
    for path in ["/api/v3/queue", "/api/v3/history"] {
        let body: Value = server
            .client()
            .get(server.url(path))
            .send()
            .await
            .expect("request")
            .json()
            .await
            .expect("json");
        for k in [
            "page",
            "pageSize",
            "sortKey",
            "sortDirection",
            "totalRecords",
            "records",
        ] {
            assert!(body.get(k).is_some(), "{path} missing {k}");
        }
    }
}

#[tokio::test]
async fn command_list_get_works() {
    let server = start_authed().await;
    seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;
    // Submit one command so the list is non-trivial.
    server
        .client()
        .post(server.url("/api/v3/command"))
        .header("X-Api-Key", TEST_API_KEY)
        .json(&serde_json::json!({ "name": "RssSync" }))
        .send()
        .await
        .expect("request");
    let body: Value = server
        .client()
        .get(server.url("/api/v3/command"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert!(body.is_array(), "GET /command must be an array");
}

// --- two faces: list resources scoped per face -----------------------------

#[tokio::test]
async fn faces_serve_their_own_library_lists() {
    let server = start_authed().await;
    let movies = seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;
    let shows = seed_library(&server.state, cellarr_core::MediaType::Tv, "Shows").await;
    let _ = (movies, shows);

    // Add a movie via the Radarr face and a series via the Sonarr face.
    server
        .client()
        .post(server.url("/radarr/api/v3/movie"))
        .header("X-Api-Key", TEST_API_KEY)
        .json(&serde_json::json!({ "title": "Dune", "rootFolderPath": "/data", "monitored": true }))
        .send()
        .await
        .expect("request");
    server
        .client()
        .post(server.url("/sonarr/api/v3/series"))
        .header("X-Api-Key", TEST_API_KEY)
        .json(&serde_json::json!({ "title": "Severance", "rootFolderPath": "/data", "monitored": true }))
        .send()
        .await
        .expect("request");

    // Radarr face GET /movie returns movie resources with path/hasFile/monitored.
    let movie_list: Value = server
        .client()
        .get(server.url("/radarr/api/v3/movie"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    let m = &movie_list.as_array().unwrap()[0];
    assert!(m.get("path").is_some());
    assert!(m.get("rootFolderPath").is_some());
    assert!(m.get("monitored").is_some());
    assert!(m.get("hasFile").is_some());
    assert!(m.get("tmdbId").is_some(), "movie resource carries tmdbId");
    assert!(m.get("tvdbId").is_none(), "movie resource has no tvdbId");

    // Sonarr face GET /series returns series resources.
    let series_list: Value = server
        .client()
        .get(server.url("/sonarr/api/v3/series"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    let s = &series_list.as_array().unwrap()[0];
    assert!(s.get("path").is_some());
    assert!(s.get("hasFile").is_some());
    assert!(s.get("tvdbId").is_some(), "series resource carries tvdbId");

    // Sonarr face has the /episode list endpoint (empty but well-shaped).
    let eps: Value = server
        .client()
        .get(server.url("/sonarr/api/v3/episode"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert!(eps.is_array());
}

#[tokio::test]
async fn indexers_visible_on_both_faces() {
    let server = start_open().await;
    seed_indexer(&server.state, "shared").await;
    for base in ["/sonarr/api/v3", "/radarr/api/v3"] {
        let body: Value = server
            .client()
            .get(server.url(&format!("{base}/indexer")))
            .send()
            .await
            .expect("request")
            .json()
            .await
            .expect("json");
        assert_eq!(
            body.as_array().unwrap().len(),
            1,
            "{base} should see the shared indexer"
        );
    }
}
