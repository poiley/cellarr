//! `/api/v3/importlist/{id}/sync` (single-list sync trigger) and the
//! `ImportListSync` command (sync-all), driven over a FAKE
//! [`ImportListSyncRunner`] seam so the safeguard reporting (fetchSucceeded) and
//! the not-wired degradation are both exercised without a live source.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use common::{start_authed, start_with_state, TEST_API_KEY};
use serde_json::{json, Value};

use cellarr_api::import_list_sync::{ImportListSyncOutcome, ImportListSyncRunner, ListSyncReport};
use cellarr_core::{CleanAction, ImportListConfig, ImportListRepository, MediaType};

/// Seed an import list (the row the sync trigger resolves), returning its numeric
/// v3 id (read back from the list endpoint).
async fn seed_list(server: &common::TestServer, id: &str, name: &str) -> i64 {
    let cfg = ImportListConfig {
        id: id.to_string(),
        name: name.to_string(),
        kind: "tmdb".to_string(),
        enabled: true,
        media_type: MediaType::Movie,
        monitored: true,
        clean_action: CleanAction::None,
        quality_profile_id: None,
        last_successful_sync: None,
        settings: serde_json::Value::Null,
    };
    server.state.db.import_lists().upsert(&cfg).await.unwrap();
    let list: Value = server
        .client()
        .get(server.url("/api/v3/importlist"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    list.as_array()
        .unwrap()
        .iter()
        .find(|l| l["name"] == name)
        .unwrap()["id"]
        .as_i64()
        .unwrap()
}

/// A fake sync runner that records how many syncs ran and reports a configured
/// per-list outcome (so a test can drive both the confirmed-good and the
/// safeguarded failed-fetch reporting).
struct FakeSync {
    runs: Arc<AtomicUsize>,
    /// (fetch_succeeded, added) the fake reports for each synced list.
    report: (bool, usize),
}

impl FakeSync {
    fn report_for(&self, list_id: &str, list_name: &str) -> ListSyncReport {
        ListSyncReport {
            list_id: list_id.to_string(),
            list_name: list_name.to_string(),
            fetch_succeeded: self.report.0,
            added: self.report.1,
            cleaned: 0,
            failure_reason: (!self.report.0).then(|| "source unavailable".to_string()),
        }
    }
}

#[async_trait]
impl ImportListSyncRunner for FakeSync {
    async fn sync_all(&self) -> Result<ImportListSyncOutcome, String> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(ImportListSyncOutcome::Ran(vec![
            self.report_for("list-1", "Popular")
        ]))
    }

    async fn sync_one(&self, list_id: &str) -> Result<ImportListSyncOutcome, String> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        // The fake knows about "list-1"; any other id is an empty run (the shim
        // maps that to a 404), mirroring the live seam's unknown-id handling.
        if list_id == "list-1" {
            Ok(ImportListSyncOutcome::Ran(vec![
                self.report_for(list_id, "Popular")
            ]))
        } else {
            Ok(ImportListSyncOutcome::Ran(vec![]))
        }
    }
}

#[tokio::test]
async fn sync_one_triggers_the_runner_and_reports_per_list() {
    let runs = Arc::new(AtomicUsize::new(0));
    let sync = Arc::new(FakeSync {
        runs: Arc::clone(&runs),
        report: (true, 3),
    });
    let server = start_with_state(move |s| s.with_import_list_sync(sync)).await;
    let numeric = seed_list(&server, "list-1", "Popular").await;

    let resp = server
        .client()
        .post(server.url(&format!("/api/v3/importlist/{numeric}/sync")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["triggered"], true);
    let lists = body["lists"].as_array().unwrap();
    assert_eq!(lists.len(), 1);
    assert_eq!(lists[0]["fetchSucceeded"], true);
    assert_eq!(lists[0]["added"], 3);
    assert_eq!(runs.load(Ordering::SeqCst), 1, "the runner ran once");
}

#[tokio::test]
async fn sync_one_surfaces_a_failed_fetch_as_the_safeguard() {
    // A source that FAILED reports fetchSucceeded:false + added 0 — the safeguard,
    // surfaced to the FE so an unavailable list never looks like a (clean-eligible)
    // empty one.
    let sync = Arc::new(FakeSync {
        runs: Arc::new(AtomicUsize::new(0)),
        report: (false, 0),
    });
    let server = start_with_state(move |s| s.with_import_list_sync(sync)).await;
    let numeric = seed_list(&server, "list-1", "Popular").await;

    let body: Value = server
        .client()
        .post(server.url(&format!("/api/v3/importlist/{numeric}/sync")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let lists = body["lists"].as_array().unwrap();
    assert_eq!(lists[0]["fetchSucceeded"], false);
    assert_eq!(lists[0]["added"], 0);
    assert_eq!(lists[0]["failureReason"], "source unavailable");
}

#[tokio::test]
async fn sync_one_unknown_list_is_404() {
    let server = start_authed().await;
    let resp = server
        .client()
        .post(server.url("/api/v3/importlist/99999/sync"))
        .header("x-api-key", TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn sync_one_without_wiring_reports_not_triggered_not_500() {
    // No sync seam (offline default): the trigger is accepted-but-unwired, never a
    // 500.
    let server = start_authed().await;
    let numeric = seed_list(&server, "list-1", "Popular").await;
    let resp = server
        .client()
        .post(server.url(&format!("/api/v3/importlist/{numeric}/sync")))
        .header("x-api-key", TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["triggered"], false);
}

#[tokio::test]
async fn import_list_sync_command_syncs_all() {
    let runs = Arc::new(AtomicUsize::new(0));
    let sync = Arc::new(FakeSync {
        runs: Arc::clone(&runs),
        report: (true, 5),
    });
    let server = start_with_state(move |s| s.with_import_list_sync(sync)).await;

    let resp = server
        .client()
        .post(server.url("/api/v3/command"))
        .json(&json!({ "name": "ImportListSync" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["commandName"], "ImportListSync");
    assert_eq!(body["result"]["triggered"], true);
    assert_eq!(body["result"]["lists"].as_array().unwrap().len(), 1);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

/// The command path needs auth on the write route. Confirm the `ImportListSync`
/// command name still goes through the authed `/command` route.
#[tokio::test]
async fn import_list_sync_command_requires_api_key() {
    let server = start_authed().await;
    let resp = server
        .client()
        .post(server.url("/api/v3/command"))
        .json(&json!({ "name": "ImportListSync" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn import_list_sync_command_without_wiring_is_not_triggered() {
    // The offline state has no sync seam: the command still answers (no 500) and
    // reports the sync-all as not-triggered.
    let server = start_authed().await;
    let body: Value = server
        .client()
        .post(server.url("/api/v3/command"))
        .header("x-api-key", TEST_API_KEY)
        .json(&json!({ "name": "ImportListSync" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["result"]["triggered"], false);
}
