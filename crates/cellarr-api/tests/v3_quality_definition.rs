//! Contract tests for the editable `/api/v3/qualitydefinition` surface.
//!
//! `GET /qualitydefinition` exposes the quality catalogue with its per-quality
//! size limits and titles; `PUT /qualitydefinition/{id}` (and the bulk
//! `PUT /qualitydefinition/update`) edit a quality's title + min/max/preferred
//! size-per-minute. These assert that an edit persists, that a subsequent GET
//! reflects it, and that the stored ranking the decision engine reads carries the
//! edited bounds.

mod common;

use common::start_open;
use serde_json::{json, Value};

/// Fetch the GET list and return the entry for a given quality `name`.
async fn definition_named(server: &common::TestServer, name: &str) -> Value {
    let body: Value = server
        .client()
        .get(server.url("/api/v3/qualitydefinition"))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .expect("json");
    body.as_array()
        .expect("array")
        .iter()
        .find(|q| q["quality"]["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("quality {name} not present"))
}

#[tokio::test]
async fn put_quality_definition_persists_and_get_reflects_edit() {
    let server = start_open().await;
    let client = server.client();

    // Read the current Bluray-1080p entry; defaults have no minimum and a null
    // preferred size.
    let before = definition_named(&server, "Bluray-1080p").await;
    let id = before["id"].as_i64().expect("numeric id");
    assert_eq!(
        before["minSize"],
        json!(0),
        "default minimum is the 0 sentinel"
    );
    assert_eq!(before["preferredSize"], Value::Null);

    // PUT new title + size bounds + preferred.
    let updated: Value = client
        .put(server.url(&format!("/api/v3/qualitydefinition/{id}")))
        .json(&json!({
            "id": id,
            "title": "HD Bluray",
            "minSize": 10,
            "maxSize": 200,
            "preferredSize": 100,
        }))
        .send()
        .await
        .expect("put")
        .json()
        .await
        .expect("put json");
    assert_eq!(updated["id"], json!(id), "id preserved");
    assert_eq!(updated["title"], "HD Bluray");
    assert_eq!(updated["minSize"], json!(10));
    assert_eq!(updated["maxSize"], json!(200));
    assert_eq!(updated["preferredSize"], json!(100));
    // The canonical quality name never changes — only the display title.
    assert_eq!(updated["quality"]["name"], "Bluray-1080p");

    // A fresh GET reflects the persisted edit.
    let after = definition_named(&server, "Bluray-1080p").await;
    assert_eq!(after["title"], "HD Bluray");
    assert_eq!(after["minSize"], json!(10));
    assert_eq!(after["maxSize"], json!(200));
    assert_eq!(after["preferredSize"], json!(100));

    // The stored ranking the decision engine reads carries the edited bounds.
    let ranking = server
        .state
        .db
        .profiles()
        .quality_ranking()
        .await
        .expect("quality_ranking");
    let def = ranking
        .by_name("Bluray-1080p")
        .and_then(|q| ranking.definition_for_rank(q.rank).cloned())
        .expect("definition present");
    assert_eq!(def.min_size_per_min, Some(10));
    assert_eq!(def.max_size_per_min, Some(200));
    assert_eq!(def.preferred_size_per_min, Some(100));
    assert_eq!(def.display_title(), "HD Bluray");

    // Other buckets are untouched.
    let untouched = definition_named(&server, "WEBDL-1080p").await;
    assert_eq!(untouched["minSize"], json!(0));
    assert_eq!(untouched["title"], "WEBDL-1080p");
}

#[tokio::test]
async fn bulk_put_updates_many_definitions() {
    let server = start_open().await;
    let client = server.client();

    let bluray = definition_named(&server, "Bluray-1080p").await;
    let webdl = definition_named(&server, "WEBDL-1080p").await;
    let bluray_id = bluray["id"].as_i64().unwrap();
    let webdl_id = webdl["id"].as_i64().unwrap();

    let out: Value = client
        .put(server.url("/api/v3/qualitydefinition/update"))
        .json(&json!([
            { "id": bluray_id, "minSize": 5, "maxSize": 50 },
            { "id": webdl_id, "minSize": 1, "maxSize": 20 },
        ]))
        .send()
        .await
        .expect("bulk put")
        .json()
        .await
        .expect("bulk json");
    assert_eq!(out.as_array().expect("array").len(), 2);

    let bluray_after = definition_named(&server, "Bluray-1080p").await;
    let webdl_after = definition_named(&server, "WEBDL-1080p").await;
    assert_eq!(bluray_after["maxSize"], json!(50));
    assert_eq!(webdl_after["maxSize"], json!(20));
}

#[tokio::test]
async fn put_unknown_id_is_404_and_negative_size_is_400() {
    let server = start_open().await;
    let client = server.client();

    // An id past the catalogue is a 404.
    let resp = client
        .put(server.url("/api/v3/qualitydefinition/99999"))
        .json(&json!({ "minSize": 1 }))
        .send()
        .await
        .expect("put");
    assert_eq!(resp.status(), 404);

    // A negative size is a 400.
    let bluray = definition_named(&server, "Bluray-1080p").await;
    let id = bluray["id"].as_i64().unwrap();
    let resp = client
        .put(server.url(&format!("/api/v3/qualitydefinition/{id}")))
        .json(&json!({ "id": id, "minSize": -5 }))
        .send()
        .await
        .expect("put");
    assert_eq!(resp.status(), 400);
}
