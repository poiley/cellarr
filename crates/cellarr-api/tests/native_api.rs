//! Handler tests for the native `/api/v1` API: request → response, auth, and
//! the structured error-body shape.

mod common;

use common::{
    seed_download_client, seed_indexer, seed_library, start_authed, start_open, TEST_API_KEY,
};
use serde_json::Value;

#[tokio::test]
async fn system_status_is_open_and_reports_counts() {
    let server = start_open().await;
    seed_library(&server.state, cellarr_core::MediaType::Movie, "Movies").await;
    seed_indexer(&server.state, "idx").await;

    let resp = server
        .client()
        .get(server.url("/api/v1/system/status"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["app_name"], "cellarr");
    assert_eq!(body["library_count"], 1);
    assert_eq!(body["indexer_count"], 1);
    assert_eq!(body["auth_enabled"], false);
}

#[tokio::test]
async fn list_libraries_returns_seeded() {
    let server = start_open().await;
    seed_library(&server.state, cellarr_core::MediaType::Tv, "Shows").await;

    let body: Value = server
        .client()
        .get(server.url("/api/v1/libraries"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(body.as_array().expect("array").len(), 1);
    assert_eq!(body[0]["name"], "Shows");
    assert_eq!(body[0]["media_type"], "tv");
}

#[tokio::test]
async fn get_missing_library_returns_structured_not_found() {
    let server = start_open().await;
    let id = uuid::Uuid::new_v4();
    let resp = server
        .client()
        .get(server.url(&format!("/api/v1/libraries/{id}")))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 404);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["code"], "not_found");
    assert!(body["message"]
        .as_str()
        .expect("message")
        .contains("not found"));
}

#[tokio::test]
async fn malformed_id_is_bad_request_not_not_found() {
    let server = start_open().await;
    let resp = server
        .client()
        .get(server.url("/api/v1/libraries/not-a-uuid"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["code"], "bad_request");
}

#[tokio::test]
async fn create_library_requires_api_key_when_enabled() {
    let server = start_authed().await;
    let profile_id = common::seed_profile(&server.state, "p").await;
    let body = serde_json::json!({
        "media_type": "movie",
        "name": "Movies",
        "root_folders": ["/data"],
        "default_quality_profile": profile_id.to_string(),
    });

    // Without a key: 401 with a structured body.
    let resp = server
        .client()
        .post(server.url("/api/v1/libraries"))
        .json(&body)
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 401);
    let err: Value = resp.json().await.expect("json");
    assert_eq!(err["code"], "unauthorized");

    // With the key: created.
    let resp = server
        .client()
        .post(server.url("/api/v1/libraries"))
        .header("X-Api-Key", TEST_API_KEY)
        .json(&body)
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 200);
    let created: Value = resp.json().await.expect("json");
    assert_eq!(created["name"], "Movies");
}

#[tokio::test]
async fn api_key_accepted_via_query_param() {
    // The ecosystem sends ?apikey=; the same middleware guards native writes.
    let server = start_authed().await;
    let profile_id = common::seed_profile(&server.state, "p").await;
    let body = serde_json::json!({
        "media_type": "movie",
        "name": "Movies",
        "default_quality_profile": profile_id.to_string(),
    });
    let resp = server
        .client()
        .post(server.url(&format!("/api/v1/libraries?apikey={TEST_API_KEY}")))
        .json(&body)
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn create_indexer_and_download_client_round_trip() {
    let server = start_open().await;
    let indexer = seed_indexer(&server.state, "x").await;
    let client = seed_download_client(&server.state, "y").await;

    let indexers: Value = server
        .client()
        .get(server.url("/api/v1/indexers"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert!(indexers
        .as_array()
        .expect("array")
        .iter()
        .any(|i| i["id"] == indexer.id.to_string()));

    let clients: Value = server
        .client()
        .get(server.url("/api/v1/downloadclients"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert!(clients
        .as_array()
        .expect("array")
        .iter()
        .any(|c| c["id"] == client.id.to_string()));
}

#[tokio::test]
async fn run_command_queues_a_job() {
    let server = start_open().await;
    let resp = server
        .client()
        .post(server.url("/api/v1/commands"))
        .json(&serde_json::json!({ "name": "RssSync" }))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["status"], "queued");
    assert_eq!(body["name"], "RssSync");
    assert!(!body["job_id"].as_str().expect("job_id").is_empty());
}

#[tokio::test]
async fn unknown_command_is_bad_request() {
    let server = start_open().await;
    let resp = server
        .client()
        .post(server.url("/api/v1/commands"))
        .json(&serde_json::json!({ "name": "NopeNotReal" }))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["code"], "bad_request");
}

#[tokio::test]
async fn openapi_spec_is_served_and_documents_routes() {
    let server = start_open().await;
    let spec: Value = server
        .client()
        .get(server.url("/api/v1/openapi.json"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    assert_eq!(spec["openapi"], "3.1.0");
    // Every native path is present.
    for (path, _methods) in cellarr_api::openapi::NATIVE_PATHS {
        assert!(
            spec["paths"].get(path).is_some(),
            "openapi spec missing path {path}"
        );
    }
    // Mutating ops carry the API-key security requirement.
    let post_sec = &spec["paths"]["/api/v1/libraries"]["post"]["security"];
    assert!(!post_sec.as_array().expect("security array").is_empty());
}

#[tokio::test]
async fn unbuilt_ui_serves_placeholder() {
    let server = start_open().await;
    let resp = server
        .client()
        .get(server.url("/"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 200);
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(ctype.starts_with("text/html"), "got {ctype}");
    let body = resp.text().await.expect("text");
    assert!(body.contains("UI has not been built") || body.contains("cellarr"));
}
