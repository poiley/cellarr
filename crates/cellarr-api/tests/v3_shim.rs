//! Contract tests for the `/api/v3` Radarr/Sonarr compatibility shim.
//!
//! These assert the shim presents the **documented** v3 request/response shapes
//! that ecosystem clients (Overseerr/Jellyseerr, Notifiarr) actually read, and
//! that it picks the right app surface per library type: Radarr-like for a movie
//! library, Sonarr-like for a TV library. The fixtures here are synthetic but
//! mirror the documented v3 field names — they are the contract.

mod common;

use common::{seed_library, start_authed, start_open, TEST_API_KEY};
use serde_json::Value;

// --- system/status ---------------------------------------------------------

#[tokio::test]
async fn status_presents_radarr_for_movie_library() {
    let server = start_open().await;
    seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;

    let body: Value = server
        .client()
        .get(server.url("/api/v3/system/status"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(body["appName"], "Radarr");
    // Fields Overseerr reads off status.
    assert!(body.get("version").is_some());
    assert!(body.get("instanceName").is_some());
}

#[tokio::test]
async fn status_presents_sonarr_for_tv_library() {
    let server = start_open().await;
    seed_library(&server.state, cellarr_core::MediaType::Tv, "Shows").await;

    let body: Value = server
        .client()
        .get(server.url("/api/v3/system/status"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(body["appName"], "Sonarr");
}

// --- qualityprofile --------------------------------------------------------

#[tokio::test]
async fn qualityprofile_has_v3_shape_for_movie() {
    let server = start_open().await;
    seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;

    let body: Value = server
        .client()
        .get(server.url("/api/v3/qualityprofile"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    let arr = body.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    let p = &arr[0];
    // The v3 fields Overseerr reads when choosing a profile.
    assert!(p.get("id").is_some());
    assert!(p.get("name").is_some());
    assert!(p.get("items").is_some());
    assert!(p.get("cutoff").is_some());
    assert!(p["items"].as_array().is_some());
}

#[tokio::test]
async fn qualityprofile_has_v3_shape_for_tv() {
    let server = start_open().await;
    seed_library(&server.state, cellarr_core::MediaType::Tv, "Shows").await;

    let body: Value = server
        .client()
        .get(server.url("/api/v3/qualityprofile"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(body.as_array().expect("array").len(), 1);
}

// --- lookup ----------------------------------------------------------------

#[tokio::test]
async fn movie_lookup_returns_radarr_shaped_results() {
    let server = start_authed().await;
    let lib = seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;

    // Add a movie so a lookup can find it (also exercises POST /movie auth).
    add_item(&server, "/api/v3/movie", "Blade Runner 2049", lib).await;

    let results: Value = server
        .client()
        .get(server.url("/api/v3/movie/lookup?term=Blade"))
        .header("X-Api-Key", TEST_API_KEY)
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    let arr = results.as_array().expect("array");
    assert!(!arr.is_empty(), "lookup found nothing");
    let item = &arr[0];
    // Radarr lookup item fields.
    assert!(item.get("title").is_some());
    assert!(item.get("tmdbId").is_some());
    assert!(item.get("titleSlug").is_some());
    // Radarr items do NOT carry the Sonarr-only tvdbId.
    assert!(item.get("tvdbId").is_none());
}

#[tokio::test]
async fn series_lookup_returns_sonarr_shaped_results() {
    let server = start_authed().await;
    let lib = seed_library(&server.state, cellarr_core::MediaType::Tv, "Shows").await;
    add_item(&server, "/api/v3/series", "The Expanse", lib).await;

    let results: Value = server
        .client()
        .get(server.url("/api/v3/series/lookup?term=Expanse"))
        .header("X-Api-Key", TEST_API_KEY)
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    let arr = results.as_array().expect("array");
    assert!(!arr.is_empty(), "lookup found nothing");
    let item = &arr[0];
    // Sonarr lookup item fields.
    assert!(item.get("title").is_some());
    assert!(item.get("tvdbId").is_some());
    assert!(item.get("seriesType").is_some());
    // Sonarr items do NOT carry the Radarr-only tmdbId.
    assert!(item.get("tmdbId").is_none());
}

// --- add -------------------------------------------------------------------

#[tokio::test]
async fn add_movie_requires_api_key() {
    let server = start_authed().await;
    seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;

    // Missing key → 401.
    let resp = server
        .client()
        .post(server.url("/api/v3/movie"))
        .json(&serde_json::json!({ "title": "Dune" }))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn add_movie_returns_radarr_shape() {
    let server = start_authed().await;
    let lib = seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;
    let body = add_item(&server, "/api/v3/movie", "Dune", lib).await;
    assert_eq!(body["title"], "Dune");
    assert!(body.get("tmdbId").is_some());
    assert!(body.get("qualityProfileId").is_some());
}

#[tokio::test]
async fn add_series_returns_sonarr_shape() {
    let server = start_authed().await;
    let lib = seed_library(&server.state, cellarr_core::MediaType::Tv, "Shows").await;
    let body = add_item(&server, "/api/v3/series", "Severance", lib).await;
    assert_eq!(body["title"], "Severance");
    assert!(body.get("tvdbId").is_some());
}

// --- command ---------------------------------------------------------------

#[tokio::test]
async fn command_accepts_radarr_movie_search() {
    let server = start_authed().await;
    seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;
    let body: Value = server
        .client()
        .post(server.url("/api/v3/command"))
        .header("X-Api-Key", TEST_API_KEY)
        .json(&serde_json::json!({ "name": "MissingMoviesSearch" }))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    // The v3 command response fields the ecosystem polls on.
    assert!(body.get("id").is_some());
    assert_eq!(body["name"], "MissingMoviesSearch");
    assert_eq!(body["status"], "queued");
}

#[tokio::test]
async fn command_accepts_sonarr_episode_search() {
    let server = start_authed().await;
    seed_library(&server.state, cellarr_core::MediaType::Tv, "Shows").await;
    let body: Value = server
        .client()
        .post(server.url("/api/v3/command"))
        .header("X-Api-Key", TEST_API_KEY)
        .json(&serde_json::json!({ "name": "MissingEpisodeSearch" }))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(body["status"], "queued");
}

// --- calendar / queue / history --------------------------------------------

#[tokio::test]
async fn calendar_queue_history_have_v3_envelopes() {
    let server = start_open().await;
    seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;

    // Calendar is a bare array.
    let cal: Value = server
        .client()
        .get(server.url("/api/v3/calendar"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert!(cal.is_array());

    // Queue and history are the paged { records: [] } envelope.
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
        assert!(body.get("records").is_some(), "{path} missing records");
        assert!(
            body.get("totalRecords").is_some(),
            "{path} missing totalRecords"
        );
        assert!(body["records"].is_array());
    }
}

// --- helper ----------------------------------------------------------------

/// POST an add request with the API key and return the parsed body.
async fn add_item(
    server: &common::TestServer,
    path: &str,
    title: &str,
    _library: cellarr_core::LibraryId,
) -> Value {
    let resp = server
        .client()
        .post(server.url(path))
        .header("X-Api-Key", TEST_API_KEY)
        .json(&serde_json::json!({
            "title": title,
            "rootFolderPath": "/data",
            "monitored": true,
        }))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 200, "add {title} failed");
    resp.json().await.expect("json")
}
