//! `/api/v3/notification` (Connect webhook) CRUD + schema + test, and
//! `/api/v3/blocklist` (list + delete), on the cellarr/Sonarr/Radarr faces.
//!
//! HERMETIC: the in-memory-shaped test server is the standard harness; the
//! `notification/test` path delivers a real `Test` Connect webhook to a LOCAL
//! mock HTTP server (a tokio TCP listener on an OS-allocated port) and asserts the
//! mock received `eventType == "Test"` — no external service.

mod common;

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use cellarr_core::blocklist::{BlocklistEntry, BlocklistRepository};
use cellarr_core::{ContentId, IndexerId, Protocol, Release};

// --- local mock HTTP server (records POST bodies) --------------------------

struct Received {
    body: Value,
}

async fn spawn_mock_server() -> (String, Arc<Mutex<Vec<Received>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let received: Arc<Mutex<Vec<Received>>> = Arc::new(Mutex::new(Vec::new()));
    let recv = Arc::clone(&received);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let recv = Arc::clone(&recv);
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(body) = parse_body(&buf) {
                        recv.lock().unwrap().push(Received { body });
                        break;
                    }
                }
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    (format!("http://127.0.0.1:{port}/hook"), received)
}

fn parse_body(buf: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(buf);
    let header_end = text.find("\r\n\r\n")?;
    let head = &text[..header_end];
    let len = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    let body_bytes = &buf[header_end + 4..];
    if body_bytes.len() < len {
        return None;
    }
    serde_json::from_slice(&body_bytes[..len]).ok()
}

async fn wait_for(received: &Arc<Mutex<Vec<Received>>>, n: usize) {
    for _ in 0..200 {
        if received.lock().unwrap().len() >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {n} webhook deliveries");
}

// --- helpers ---------------------------------------------------------------

fn release(title: &str, guid: &str) -> Release {
    Release {
        indexer_id: IndexerId::new(),
        title: title.into(),
        download_url: format!("magnet:?xt={guid}"),
        guid: Some(guid.into()),
        protocol: Protocol::Torrent,
        size: Some(1_000),
        seeders: Some(5),
        indexer_flags: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Notification CRUD + schema
// ---------------------------------------------------------------------------

#[tokio::test]
async fn notification_schema_advertises_the_webhook_connector() {
    let server = common::start_open().await;
    let resp = server
        .client()
        .get(server.url("/api/v3/notification/schema"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let arr = body.as_array().unwrap();
    assert!(arr
        .iter()
        .any(|t| t["implementation"] == "Webhook" && t["supportsOnGrab"] == true));
}

#[tokio::test]
async fn notification_create_list_update_delete_roundtrip() {
    let server = common::start_open().await;
    let client = server.client();

    // CREATE a webhook notification.
    let created: Value = client
        .post(server.url("/api/v3/notification"))
        .json(&json!({
            "name": "my-hook",
            "implementation": "Webhook",
            "onGrab": true,
            "onDownload": true,
            "onRename": false,
            "onHealthIssue": true,
            "fields": [ { "name": "url", "value": "http://example.invalid/hook" } ],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["implementation"], "Webhook");
    assert_eq!(created["onGrab"], true);
    assert_eq!(created["onRename"], false);
    let id = created["id"].as_i64().unwrap();

    // LIST shows it.
    let list: Value = client
        .get(server.url("/api/v3/notification"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);

    // GET by id.
    let got: Value = client
        .get(server.url(&format!("/api/v3/notification/{id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got["name"], "my-hook");

    // UPDATE the name + toggle rename on.
    let updated: Value = client
        .put(server.url(&format!("/api/v3/notification/{id}")))
        .json(&json!({
            "name": "renamed-hook",
            "implementation": "Webhook",
            "onGrab": true,
            "onDownload": true,
            "onRename": true,
            "onHealthIssue": true,
            "fields": [ { "name": "url", "value": "http://example.invalid/hook" } ],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["name"], "renamed-hook");
    assert_eq!(updated["onRename"], true);

    // DELETE disables it (it stops appearing as an active webhook target).
    let del = client
        .delete(server.url(&format!("/api/v3/notification/{id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 200);
    let disabled = server.state.db.config().list_notifications().await.unwrap();
    assert!(disabled.iter().all(|n| !n.enabled));
}

#[tokio::test]
async fn notification_test_delivers_a_test_event_to_the_mock() {
    let (url, received) = spawn_mock_server().await;
    let server = common::start_open().await;

    let resp = server
        .client()
        .post(server.url("/api/v3/notification/test"))
        .json(&json!({
            "name": "probe",
            "implementation": "Webhook",
            "fields": [ { "name": "url", "value": url } ],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["isValid"], true);

    wait_for(&received, 1).await;
    let calls = received.lock().unwrap();
    assert_eq!(calls[0].body["eventType"], "Test");
}

#[tokio::test]
async fn notification_test_reports_failure_for_an_unreachable_url() {
    let server = common::start_open().await;
    // A port nothing listens on: delivery fails -> isValid:false (not a 500).
    let resp = server
        .client()
        .post(server.url("/api/v3/notification/test"))
        .json(&json!({
            "name": "probe",
            "implementation": "Webhook",
            "fields": [ { "name": "url", "value": "http://127.0.0.1:1/hook" } ],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["isValid"], false);
    assert!(!body["validationFailures"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn notification_test_missing_url_is_a_400() {
    let server = common::start_open().await;
    let resp = server
        .client()
        .post(server.url("/api/v3/notification/test"))
        .json(&json!({ "name": "probe", "implementation": "Webhook", "fields": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

// ---------------------------------------------------------------------------
// Blocklist list + delete
// ---------------------------------------------------------------------------

/// Seed a content node (so the blocklist FK is satisfied) and a blocklist entry.
async fn seed_blocklisted(
    server: &common::TestServer,
    title: &str,
    guid: &str,
) -> (ContentId, String) {
    use cellarr_core::repo::ContentRepository;
    let library_id =
        common::seed_library(&server.state, cellarr_core::MediaType::Movie, "lib").await;
    let content_id = ContentId::new();
    let node = cellarr_core::ContentNode {
        id: content_id,
        library_id,
        media_type: cellarr_core::MediaType::Movie,
        parent_id: None,
        kind: cellarr_core::ContentKind::Movie,
        coords: cellarr_core::Coordinates::Movie,
        monitored: true,
        title_id: None,
    };
    server.state.db.content().upsert(&node).await.unwrap();

    let entry = BlocklistEntry::from_release(
        content_id,
        &release(title, guid),
        "download failed",
        time::OffsetDateTime::now_utc(),
    );
    let id = entry.id.clone();
    BlocklistRepository::add(&server.state.db.blocklist(), &entry)
        .await
        .unwrap();
    (content_id, id)
}

#[tokio::test]
async fn blocklist_lists_then_deletes_an_entry() {
    let server = common::start_open().await;
    seed_blocklisted(&server, "Bad.Release.1080p-X", "guid-1").await;

    // GET /api/v3/blocklist returns the paged record.
    let list: Value = server
        .client()
        .get(server.url("/api/v3/blocklist"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["totalRecords"], 1);
    let rec = &list["records"][0];
    assert_eq!(rec["sourceTitle"], "Bad.Release.1080p-X");
    let id = rec["id"].as_i64().unwrap();

    // DELETE clears it.
    let del = server
        .client()
        .delete(server.url(&format!("/api/v3/blocklist/{id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 200);

    let after: Value = server
        .client()
        .get(server.url("/api/v3/blocklist"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["totalRecords"], 0);
}

#[tokio::test]
async fn blocklist_bulk_delete_clears_selected() {
    let server = common::start_open().await;
    seed_blocklisted(&server, "A.1080p-X", "guid-a").await;
    seed_blocklisted(&server, "B.1080p-Y", "guid-b").await;

    let list: Value = server
        .client()
        .get(server.url("/api/v3/blocklist"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<i64> = list["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);

    let del = server
        .client()
        .delete(server.url("/api/v3/blocklist/bulk"))
        .json(&json!({ "ids": ids }))
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 200);

    let after: Value = server
        .client()
        .get(server.url("/api/v3/blocklist"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["totalRecords"], 0);
}

#[tokio::test]
async fn blocklist_delete_is_idempotent_on_unknown_id() {
    let server = common::start_open().await;
    let del = server
        .client()
        .delete(server.url("/api/v3/blocklist/999999"))
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 200);
}

#[tokio::test]
async fn notification_endpoints_present_on_sonarr_and_radarr_faces() {
    let server = common::start_open().await;
    for face in ["/sonarr/api/v3", "/radarr/api/v3"] {
        let resp = server
            .client()
            .get(server.url(&format!("{face}/notification/schema")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "schema missing on {face}");
        let resp = server
            .client()
            .get(server.url(&format!("{face}/blocklist")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "blocklist missing on {face}");
    }
}
