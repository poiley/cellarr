//! `/api/v3` tag round-trip tests: the persisted tag vocabulary plus tags on
//! movie/series, indexer, download-client, and notification bodies.
//!
//! Asserts:
//!   - `POST /api/v3/tag` mints a persisted `{id,label}` the list/get surface;
//!   - `POST /api/v3/movie` and `/series` accept a `tags:[int]` array, persist it,
//!     and return it; a `PUT` rewrites it (and an omitted `tags` keeps it);
//!   - indexer / download-client / notification bodies accept + return `tags`.

mod common;

use common::start_open;
use serde_json::json;

/// Create a tag via the v3 surface, returning its assigned id.
async fn create_tag(server: &common::TestServer, label: &str) -> u32 {
    let resp = server
        .client()
        .post(server.url("/api/v3/tag"))
        .json(&json!({ "label": label }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "tag create should succeed");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["id"].as_u64().unwrap() as u32
}

#[tokio::test]
async fn tag_crud_persists_and_dedups() {
    let server = start_open().await;
    let anime = create_tag(&server, "Anime").await;
    let uhd = create_tag(&server, "4K").await;
    assert_eq!(anime, 1);
    assert_eq!(uhd, 2);
    // A case-insensitive duplicate returns the existing id.
    assert_eq!(create_tag(&server, "anime").await, anime);

    // The list surface returns both, persisted.
    let resp = server
        .client()
        .get(server.url("/api/v3/tag"))
        .send()
        .await
        .unwrap();
    let list: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn movie_tags_round_trip_on_add_and_update() {
    let server = start_open().await;
    common::seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;
    let anime = create_tag(&server, "anime").await;
    let uhd = create_tag(&server, "4k").await;

    // POST a movie with tags -> persisted + returned.
    let resp = server
        .client()
        .post(server.url("/api/v3/movie"))
        .json(&json!({ "title": "Akira", "tags": [anime, uhd] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let id = body["id"].as_str().unwrap().to_string();
    let mut tags: Vec<u64> = body["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    tags.sort_unstable();
    assert_eq!(tags, vec![u64::from(anime), u64::from(uhd)]);

    // GET the detail back -> same tags.
    let resp = server
        .client()
        .get(server.url(&format!("/api/v3/movie/{id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let detail: serde_json::Value = resp.json().await.unwrap();
    let mut got: Vec<u64> = detail["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    got.sort_unstable();
    assert_eq!(got, vec![u64::from(anime), u64::from(uhd)]);

    // PUT with tags rewrites them (drop uhd, keep anime).
    let resp = server
        .client()
        .put(server.url(&format!("/api/v3/movie/{id}")))
        .json(&json!({ "tags": [anime] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let got: Vec<u64> = body["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    assert_eq!(got, vec![u64::from(anime)], "PUT rewrites the tag set");

    // A PUT that omits tags (only flips monitored) must NOT drop them.
    let resp = server
        .client()
        .put(server.url(&format!("/api/v3/movie/{id}")))
        .json(&json!({ "monitored": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["tags"].as_array().unwrap().len(),
        1,
        "omitting tags on PUT keeps the existing tags"
    );
}

#[tokio::test]
async fn series_tags_round_trip_on_add() {
    let server = start_open().await;
    common::seed_library(&server.state, cellarr_core::MediaType::Tv, "TV").await;
    let anime = create_tag(&server, "anime").await;

    let resp = server
        .client()
        .post(server.url("/api/v3/series"))
        .json(&json!({ "title": "Cowboy Bebop", "tags": [anime] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let got: Vec<u64> = body["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    assert_eq!(got, vec![u64::from(anime)]);
}

#[tokio::test]
async fn indexer_download_client_notification_tags_round_trip() {
    let server = start_open().await;
    let anime = create_tag(&server, "anime").await;

    // Indexer: push a tagged body, read the tag back.
    let resp = server
        .client()
        .post(server.url("/api/v3/indexer"))
        .json(&json!({
            "name": "Mock",
            "implementation": "Torznab",
            "protocol": "torrent",
            "tags": [anime],
            "fields": [ { "name": "baseUrl", "value": "http://localhost" } ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ix: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        ix["tags"].as_array().unwrap(),
        &vec![serde_json::json!(anime)]
    );

    // Download client.
    let resp = server
        .client()
        .post(server.url("/api/v3/downloadclient"))
        .json(&json!({
            "name": "qbit",
            "implementation": "QBittorrent",
            "protocol": "torrent",
            "tags": [anime],
            "fields": [ { "name": "category", "value": "cellarr" } ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let dc: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        dc["tags"].as_array().unwrap(),
        &vec![serde_json::json!(anime)]
    );

    // Notification.
    let resp = server
        .client()
        .post(server.url("/api/v3/notification"))
        .json(&json!({
            "name": "wh",
            "implementation": "Webhook",
            "tags": [anime],
            "fields": [ { "name": "url", "value": "http://localhost/hook" } ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let n: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        n["tags"].as_array().unwrap(),
        &vec![serde_json::json!(anime)]
    );
}
