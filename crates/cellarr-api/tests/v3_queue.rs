//! `/api/v3/queue` management: list (real grabs), remove (+removeFromClient /
//! +blocklist), change category, and grab-from-queue (manual import).
//!
//! HERMETIC: the file-backed test server harness; no live services. The queue is
//! backed by real `grab` rows seeded directly into the db. The download-client and
//! manual-import seams are FAKES injected via [`start_with_state`], so a
//! `removeFromClient` and a queue grab-import are exercised end to end without a
//! live client/pipeline.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use common::{start_authed, start_with_state, TEST_API_KEY};
use serde_json::Value;

use cellarr_api::manual_import::{
    ManualImport, ManualImportCommitOutcome, ManualImportOutcome, ManualImportRequest,
    ManualImportResult,
};
use cellarr_api::queue::QueueDownloadClient;
use cellarr_api::AppState;
use cellarr_core::repo::{ContentRepository, GrabRepository};
use cellarr_core::{
    blocklist::BlocklistRepository, ContentId, ContentKind, ContentNode, ContentRef, Coordinates,
    DownloadClientId, GrabId, GrabRequest, GrabStatus, IndexerId, MediaType, Protocol, Release,
};

/// Seed an in-flight grab (a queue item) for a movie, returning its id. The grab's
/// content ref points at a real content node in a real library so the blocklist
/// FK (`blocklist.content_id REFERENCES content(id)`) is satisfiable.
async fn seed_grab(state: &AppState, title: &str) -> GrabId {
    let library = common::seed_library(state, MediaType::Movie, &format!("lib-{title}")).await;
    let node = ContentNode {
        tags: Vec::new(),
        id: ContentId::new(),
        library_id: library,
        media_type: MediaType::Movie,
        parent_id: None,
        kind: ContentKind::Movie,
        series_type: cellarr_core::SeriesType::Standard,
        coords: Coordinates::Movie,
        monitored: true,
        title_id: None,
    };
    state.db.content().upsert(&node).await.unwrap();
    let content_ref =
        ContentRef::new(node.id, library, MediaType::Movie, Coordinates::Movie).unwrap();
    let release = Release {
        indexer_id: IndexerId::new(),
        title: title.to_string(),
        download_url: "magnet:?xt=urn:btih:abc".to_string(),
        guid: Some(format!("guid-{title}")),
        protocol: Protocol::Torrent,
        size: Some(1_000_000),
        seeders: Some(10),
        indexer_flags: vec![],
    };
    let request = GrabRequest {
        content_ref,
        release,
        indexer_id: IndexerId::new(),
        client_id: DownloadClientId::new(),
        category: "cellarr".to_string(),
        release_type: None,
    };
    let id = state.db.grabs().create(&request).await.unwrap();
    // Advance it to Sent with a download id so it is a live, removable queue item.
    state
        .db
        .grabs()
        .set_download_id(id, &format!("dl-{title}"))
        .await
        .unwrap();
    state
        .db
        .grabs()
        .set_status(id, GrabStatus::Sent)
        .await
        .unwrap();
    id
}

/// Fetch the queue records array.
async fn queue_records(server: &common::TestServer) -> Vec<Value> {
    let body: Value = server
        .client()
        .get(server.url("/api/v3/queue"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    body["records"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn queue_lists_inflight_grabs_not_terminal_ones() {
    let server = start_authed().await;
    seed_grab(&server.state, "Live One").await;
    let imported = seed_grab(&server.state, "Imported Two").await;
    // An imported grab is terminal — it must NOT show as a live queue item.
    server
        .state
        .db
        .grabs()
        .set_status(imported, GrabStatus::Imported)
        .await
        .unwrap();

    let records = queue_records(&server).await;
    assert_eq!(records.len(), 1, "only the in-flight grab is a queue item");
    assert_eq!(records[0]["title"], "Live One");
    assert_eq!(records[0]["protocol"], "torrent");
    assert_eq!(records[0]["category"], "cellarr");
}

#[tokio::test]
async fn queue_remove_deletes_the_grab() {
    let server = start_authed().await;
    let id = seed_grab(&server.state, "To Remove").await;
    let numeric = queue_records(&server).await[0]["id"].as_i64().unwrap();

    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/queue/{numeric}")))
        .header("x-api-key", TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["removed"], true);

    // The grab row is gone, so the queue is empty.
    assert!(queue_records(&server).await.is_empty());
    assert!(server.state.db.grabs().get(id).await.unwrap().is_none());
}

#[tokio::test]
async fn queue_remove_with_blocklist_adds_a_blocklist_entry() {
    let server = start_authed().await;
    seed_grab(&server.state, "Bad Release").await;
    let numeric = queue_records(&server).await[0]["id"].as_i64().unwrap();

    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/queue/{numeric}?blocklist=true")))
        .header("x-api-key", TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["removed"], true);
    assert_eq!(body["blocklisted"], true);

    // The release is now on the blocklist so a re-search never re-grabs it.
    let entries = BlocklistRepository::list(&server.state.db.blocklist())
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].title, "Bad Release");
}

/// A fake [`QueueDownloadClient`] that records whether it was asked to remove a
/// download (and with which delete-data flag).
struct RecordingQueueClient {
    removed: Arc<AtomicBool>,
    delete_data: Arc<AtomicBool>,
}

#[async_trait]
impl QueueDownloadClient for RecordingQueueClient {
    async fn remove(&self, _download_id: &str, delete_data: bool) -> Result<(), String> {
        self.removed.store(true, Ordering::SeqCst);
        self.delete_data.store(delete_data, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn queue_remove_from_client_calls_the_client_seam() {
    let removed = Arc::new(AtomicBool::new(false));
    let delete_data = Arc::new(AtomicBool::new(false));
    let client = Arc::new(RecordingQueueClient {
        removed: Arc::clone(&removed),
        delete_data: Arc::clone(&delete_data),
    });
    let server = start_with_state(move |s| s.with_queue_client(client)).await;
    seed_grab(&server.state, "Client Removal").await;
    let numeric = queue_records(&server).await[0]["id"].as_i64().unwrap();

    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/queue/{numeric}?removeFromClient=true")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["removedFromClient"], true);
    assert!(removed.load(Ordering::SeqCst), "the client seam was called");
    assert!(
        delete_data.load(Ordering::SeqCst),
        "a queue remove deletes the client's data"
    );
}

#[tokio::test]
async fn queue_remove_from_client_without_wiring_still_removes_the_row() {
    // No queue-client seam (the offline default): a removeFromClient request still
    // removes the queue row (the queue is cellarr's own state) and reports the
    // client removal not-performed.
    let server = start_authed().await;
    seed_grab(&server.state, "No Client").await;
    let numeric = queue_records(&server).await[0]["id"].as_i64().unwrap();

    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/queue/{numeric}?removeFromClient=true")))
        .header("x-api-key", TEST_API_KEY)
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["removed"], true);
    assert_eq!(body["removedFromClient"], false);
    assert!(queue_records(&server).await.is_empty());
}

#[tokio::test]
async fn queue_change_category_retags_the_download() {
    let server = start_authed().await;
    let id = seed_grab(&server.state, "Recategorize").await;
    let numeric = queue_records(&server).await[0]["id"].as_i64().unwrap();

    let resp = server
        .client()
        .put(server.url(&format!("/api/v3/queue/{numeric}")))
        .header("x-api-key", TEST_API_KEY)
        .json(&serde_json::json!({ "category": "movies-4k" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["category"], "movies-4k");

    // The change persisted on the grab.
    let grab = server.state.db.grabs().get(id).await.unwrap().unwrap();
    assert_eq!(grab.request.category, "movies-4k");
}

/// A fake [`ManualImport`] that records the committed requests and reports a
/// successful import of each.
struct FakeManualImport {
    commits: Arc<AtomicUsize>,
}

#[async_trait]
impl ManualImport for FakeManualImport {
    async fn scan(&self, _folder: &str) -> Result<ManualImportOutcome, String> {
        Ok(ManualImportOutcome::Found(vec![]))
    }

    async fn commit(
        &self,
        items: Vec<ManualImportRequest>,
    ) -> Result<ManualImportCommitOutcome, String> {
        self.commits.fetch_add(items.len(), Ordering::SeqCst);
        let imported = items
            .into_iter()
            .map(|r| ManualImportResult {
                source_path: r.path.clone(),
                destination_path: format!("/library/{}", r.path),
                content_id: r.content_id,
            })
            .collect();
        Ok(ManualImportCommitOutcome::Committed {
            imported,
            errors: vec![],
        })
    }
}

#[tokio::test]
async fn queue_grab_imports_completed_download_via_manual_import() {
    let commits = Arc::new(AtomicUsize::new(0));
    let mi = Arc::new(FakeManualImport {
        commits: Arc::clone(&commits),
    });
    let server = start_with_state(move |s| s.with_manual_import(mi)).await;
    let id = seed_grab(&server.state, "Completed Download").await;
    // The download finished, awaiting a manual content match.
    server
        .state
        .db
        .grabs()
        .set_status(id, GrabStatus::Completed)
        .await
        .unwrap();
    let numeric = queue_records(&server).await[0]["id"].as_i64().unwrap();

    let content_id = ContentId::new();
    let resp = server
        .client()
        .post(server.url("/api/v3/queue/grab"))
        .json(&serde_json::json!({
            "id": numeric,
            "contentId": content_id.to_string(),
            "path": "/downloads/completed.mkv",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["imported"], true, "the download was imported");
    assert_eq!(commits.load(Ordering::SeqCst), 1, "one file committed");

    // The grab was marked imported + dropped from the queue.
    assert!(queue_records(&server).await.is_empty());
}

#[tokio::test]
async fn queue_grab_without_pipeline_reports_unavailable_not_500() {
    let server = start_authed().await;
    let id = seed_grab(&server.state, "No Pipeline").await;
    server
        .state
        .db
        .grabs()
        .set_status(id, GrabStatus::Completed)
        .await
        .unwrap();
    let numeric = queue_records(&server).await[0]["id"].as_i64().unwrap();

    let resp = server
        .client()
        .post(server.url("/api/v3/queue/grab"))
        .header("x-api-key", TEST_API_KEY)
        .json(&serde_json::json!({ "id": numeric, "path": "/downloads/x.mkv" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "no pipeline degrades, never 500s");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["imported"], false);
}
