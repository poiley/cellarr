//! Per-episode / per-season monitor toggle + add-monitor-option + manual-import
//! degradation tests for the `/api/v3` shim.
//!
//! Asserts:
//!   - `PUT /api/v3/episode/monitor` flips `monitored` on the addressed episode
//!     nodes and persists it;
//!   - `PUT /api/v3/season/monitor` flips the season AND cascades to its episodes;
//!   - `addOptions.monitor: "none"` on an add stores the series unmonitored, while
//!     a normal add stays monitored;
//!   - `GET/POST /api/v3/manualimport` degrade to an empty/`message` result when no
//!     pipeline is wired (the offline test state), never a 500.

mod common;

use cellarr_core::repo::ContentRepository;
use cellarr_core::{ContentId, ContentKind, ContentNode, Coordinates, MediaType};
use common::{seed_library, start_open};
use serde_json::json;

/// Seed a series -> season -> two-episode tree, all monitored, returning the ids.
async fn seed_series_tree(
    state: &cellarr_api::AppState,
    library: cellarr_core::LibraryId,
) -> (ContentId, ContentId, ContentId, ContentId) {
    let content = state.db.content();
    let series = ContentId::new();
    content
        .upsert(&ContentNode {
            id: series,
            library_id: library,
            media_type: MediaType::Tv,
            parent_id: None,
            kind: ContentKind::Series,
            coords: Coordinates::Episode {
                season: 0,
                episode: 0,
                absolute: None,
            },
            monitored: true,
            title_id: None,
        })
        .await
        .unwrap();
    let season = ContentId::new();
    content
        .upsert(&ContentNode {
            id: season,
            library_id: library,
            media_type: MediaType::Tv,
            parent_id: Some(series),
            kind: ContentKind::Season,
            coords: Coordinates::SeasonPack { season: 1 },
            monitored: true,
            title_id: None,
        })
        .await
        .unwrap();
    let e1 = ContentId::new();
    let e2 = ContentId::new();
    for (id, episode) in [(e1, 1u32), (e2, 2u32)] {
        content
            .upsert(&ContentNode {
                id,
                library_id: library,
                media_type: MediaType::Tv,
                parent_id: Some(season),
                kind: ContentKind::Episode,
                coords: Coordinates::Episode {
                    season: 1,
                    episode,
                    absolute: None,
                },
                monitored: true,
                title_id: None,
            })
            .await
            .unwrap();
    }
    (series, season, e1, e2)
}

#[tokio::test]
async fn episode_monitor_toggle_persists_for_addressed_episodes() {
    let server = start_open().await;
    let library = seed_library(&server.state, MediaType::Tv, "TV").await;
    let (_series, _season, e1, e2) = seed_series_tree(&server.state, library).await;

    // Unmonitor only e1.
    let resp = server
        .client()
        .put(server.url("/api/v3/episode/monitor"))
        .json(&json!({ "episodeIds": [e1.to_string()], "monitored": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["updated"], 1);
    assert_eq!(body["monitored"], false);

    let content = server.state.db.content();
    assert!(
        !content.get_node(e1).await.unwrap().unwrap().monitored,
        "e1 was unmonitored"
    );
    assert!(
        content.get_node(e2).await.unwrap().unwrap().monitored,
        "e2 is untouched (still monitored)"
    );

    // Re-monitor both in one call.
    let resp = server
        .client()
        .put(server.url("/api/v3/episode/monitor"))
        .json(&json!({ "episodeIds": [e1.to_string(), e2.to_string()], "monitored": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["updated"], 2);
    assert!(content.get_node(e1).await.unwrap().unwrap().monitored);
    assert!(content.get_node(e2).await.unwrap().unwrap().monitored);
}

#[tokio::test]
async fn episode_monitor_skips_unknown_ids() {
    let server = start_open().await;
    let library = seed_library(&server.state, MediaType::Tv, "TV").await;
    let (_series, _season, e1, _e2) = seed_series_tree(&server.state, library).await;

    // One real episode + one id that resolves to nothing: the real one updates, the
    // unknown is skipped (idempotent), never a 500.
    let resp = server
        .client()
        .put(server.url("/api/v3/episode/monitor"))
        .json(&json!({
            "episodeIds": [e1.to_string(), ContentId::new().to_string()],
            "monitored": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["updated"], 1, "only the real episode was updated");
}

#[tokio::test]
async fn season_monitor_toggle_cascades_to_episodes() {
    let server = start_open().await;
    let library = seed_library(&server.state, MediaType::Tv, "TV").await;
    let (_series, season, e1, e2) = seed_series_tree(&server.state, library).await;

    // Unmonitor the season — every episode beneath it follows.
    let resp = server
        .client()
        .put(server.url("/api/v3/season/monitor"))
        .json(&json!({ "seasonId": season.to_string(), "monitored": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["monitored"], false);
    assert_eq!(body["episodesUpdated"], 2);

    let content = server.state.db.content();
    assert!(!content.get_node(season).await.unwrap().unwrap().monitored);
    assert!(!content.get_node(e1).await.unwrap().unwrap().monitored);
    assert!(!content.get_node(e2).await.unwrap().unwrap().monitored);
}

#[tokio::test]
async fn season_monitor_unknown_season_is_404() {
    let server = start_open().await;
    let _library = seed_library(&server.state, MediaType::Tv, "TV").await;

    let resp = server
        .client()
        .put(server.url("/api/v3/season/monitor"))
        .json(&json!({ "seasonId": ContentId::new().to_string(), "monitored": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn add_with_monitor_none_stores_series_unmonitored() {
    let server = start_open().await;
    let _library = seed_library(&server.state, MediaType::Tv, "TV").await;

    // monitor: "none" => the added series is unmonitored.
    let resp = server
        .client()
        .post(server.url("/api/v3/series"))
        .json(&json!({
            "title": "The Wire",
            "addOptions": { "monitor": "none" }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["monitored"], false, "monitor:none adds unmonitored");

    // A normal add (no addOptions) stays monitored (the default).
    let resp = server
        .client()
        .post(server.url("/api/v3/series"))
        .json(&json!({ "title": "Deadwood" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["monitored"], true, "default add is monitored");
}

#[tokio::test]
async fn manual_import_degrades_when_no_pipeline_is_wired() {
    let server = start_open().await;

    // The offline test state has no manual-import seam wired, so the scan returns an
    // empty array (never a 500) and the commit reports a clear message.
    let resp = server
        .client()
        .get(server.url("/api/v3/manualimport?folder=/tmp/whatever"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.as_array().unwrap().is_empty(), "empty scan, not a 500");

    let resp = server
        .client()
        .post(server.url("/api/v3/manualimport"))
        .json(&json!({ "files": [{ "path": "/tmp/x.mkv", "contentId": ContentId::new().to_string() }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["imported"].as_array().unwrap().is_empty());
    assert!(
        body["message"].is_string(),
        "a clear message on no pipeline"
    );
}

#[tokio::test]
async fn manual_import_scan_requires_a_folder() {
    let server = start_open().await;
    let resp = server
        .client()
        .get(server.url("/api/v3/manualimport"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "a folder query parameter is required");
}
