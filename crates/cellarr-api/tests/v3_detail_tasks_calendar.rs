//! `/api/v3` content detail + monitor toggle, scheduled tasks (`system/task`),
//! the JSON calendar, and standalone root-folder CRUD — the small endpoints the
//! new UI consumes.
//!
//! HERMETIC: the standard file-backed test server harness; no live services and
//! no credentials. Everything round-trips through the real db + scheduler.

mod common;

use common::{seed_library, start_authed, start_open, TEST_API_KEY};
use serde_json::{json, Value};

use cellarr_core::repo::ContentRepository;
use cellarr_core::{ContentId, ContentKind, ContentNode, Coordinates, MediaType};

// --- content detail + monitor toggle ---------------------------------------

/// Seed a movie node and return its id (the v3 detail/toggle key).
async fn seed_movie(state: &cellarr_api::AppState, title: &str, monitored: bool) -> ContentId {
    let library = seed_library(state, MediaType::Movie, "Movies").await;
    let id = ContentId::new();
    let node = ContentNode {
        tags: Vec::new(),
        id,
        library_id: library,
        media_type: MediaType::Movie,
        parent_id: None,
        kind: ContentKind::Movie,
        series_type: cellarr_core::SeriesType::Standard,
        coords: Coordinates::Movie,
        monitored,
        title_id: None,
    };
    state.db.content().upsert(&node).await.unwrap();
    state.db.content().index_title(id, title).await.unwrap();
    id
}

#[tokio::test]
async fn movie_detail_returns_identity_quality_profile_and_file_state() {
    let server = start_open().await;
    let id = seed_movie(&server.state, "The Matrix", true).await;

    let resp = server
        .client()
        .get(server.url(&format!("/api/v3/movie/{id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["id"], id.to_string());
    assert_eq!(body["title"], "The Matrix");
    assert_eq!(body["monitored"], true);
    // The detail surfaces a real qualityProfileId (the library default) + the
    // file-state fields the detail screen reads.
    assert!(
        body["qualityProfileId"].is_string(),
        "detail must carry the library's qualityProfileId"
    );
    assert_eq!(body["hasFile"], false);
    assert_eq!(body["sizeOnDisk"], 0);
    assert!(body.get("overview").is_some(), "overview field is present");
}

#[tokio::test]
async fn unknown_movie_detail_is_404_json() {
    let server = start_open().await;
    let resp = server
        .client()
        .get(server.url(&format!("/api/v3/movie/{}", ContentId::new())))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn monitor_toggle_flips_and_persists() {
    let server = start_authed().await;
    let id = seed_movie(&server.state, "The Matrix", true).await;

    // Toggle monitored -> false via PUT.
    let resp = server
        .client()
        .put(server.url(&format!("/api/v3/movie/{id}")))
        .header("x-api-key", TEST_API_KEY)
        .json(&json!({ "monitored": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["monitored"], false, "the response reflects the toggle");

    // It persisted: a fresh GET reads false.
    let detail: Value = server
        .client()
        .get(server.url(&format!("/api/v3/movie/{id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["monitored"], false, "the toggle is durable");

    // And the underlying node row reflects it.
    let node = server
        .state
        .db
        .content()
        .get_node(id)
        .await
        .unwrap()
        .unwrap();
    assert!(!node.monitored);
}

/// Regression: adding a movie must persist its identity (tmdbId + year) from the
/// add payload — otherwise it has no identity and the release search finds nothing.
#[tokio::test]
async fn add_movie_persists_identity_tmdbid_and_year() {
    let server = start_authed().await;
    seed_library(&server.state, MediaType::Movie, "Movies").await;

    let created: Value = server
        .client()
        .post(server.url("/api/v3/movie"))
        .header("x-api-key", TEST_API_KEY)
        .json(&json!({
            "title": "Big Buck Bunny",
            "tmdbId": 10378,
            "year": 2008,
            "monitored": true
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["tmdbId"], 10378, "add persists tmdbId: {created}");
    assert_eq!(created["year"], 2008, "add persists year: {created}");

    // And it round-trips on a fresh GET (identity is durable, not just echoed).
    let id = created["id"].as_str().expect("movie id");
    let got: Value = server
        .client()
        .get(server.url(&format!("/api/v3/movie/{id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got["tmdbId"], 10378, "tmdbId is durable: {got}");
    assert_eq!(got["year"], 2008, "year is durable: {got}");
}

// --- system/task -----------------------------------------------------------

#[tokio::test]
async fn system_task_exposes_recurring_jobs_with_next_run_and_status() {
    let server = start_open().await;
    // The API's own scheduler has no recurring jobs until one is registered;
    // register the same RssSync cron the daemon does.
    server
        .state
        .scheduler
        .add_cron(
            cellarr_jobs::JobKind::RssSync,
            "*/15 * * * *",
            cellarr_jobs::RetryPolicy::default(),
        )
        .await
        .unwrap();

    let resp = server
        .client()
        .get(server.url("/api/v3/system/task"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let tasks: Value = resp.json().await.unwrap();
    let arr = tasks.as_array().expect("task list is an array");
    let rss = arr
        .iter()
        .find(|t| t["name"] == "RssSync")
        .expect("RssSync task is listed");
    // Interval is surfaced in minutes (15-minute cron).
    assert_eq!(rss["interval"], 15);
    // Next-run is a real ISO timestamp the countdown reads.
    let next = rss["nextExecution"].as_str().expect("nextExecution string");
    assert!(
        next.contains('T') && next.ends_with('Z'),
        "nextExecution should be an ISO-8601 UTC string, got {next}"
    );
    // The last status reflects the job's lifecycle state.
    assert!(
        rss["lastStatus"].is_string(),
        "lastStatus is present for the countdown UI"
    );
    // The task is addressable by id.
    let id = rss["id"].as_i64().expect("numeric task id");
    let one: Value = server
        .client()
        .get(server.url(&format!("/api/v3/system/task/{id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(one["name"], "RssSync");
}

// --- JSON calendar ---------------------------------------------------------

#[tokio::test]
async fn json_calendar_returns_dated_items_within_range() {
    let server = start_open().await;
    // A TV daily-coded episode carries a self-contained air date.
    let library = seed_library(&server.state, MediaType::Tv, "TV").await;
    let id = ContentId::new();
    let node = ContentNode {
        tags: Vec::new(),
        id,
        library_id: library,
        media_type: MediaType::Tv,
        parent_id: None,
        kind: ContentKind::Episode,
        series_type: cellarr_core::SeriesType::Standard,
        coords: Coordinates::Daily {
            date: "2026-07-04".into(),
        },
        monitored: true,
        title_id: None,
    };
    server.state.db.content().upsert(&node).await.unwrap();
    server
        .state
        .db
        .content()
        .index_title(id, "The Daily Show")
        .await
        .unwrap();

    // The Sonarr face addresses TV; the dated episode is in the window.
    let resp = server
        .client()
        .get(server.url("/sonarr/api/v3/calendar?start=2026-07-01&end=2026-07-31"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let rows: Value = resp.json().await.unwrap();
    let arr = rows.as_array().expect("calendar is an array");
    assert_eq!(arr.len(), 1, "the dated episode is in range: {rows}");
    assert_eq!(arr[0]["airDate"], "2026-07-04");
    assert!(arr[0]["title"].as_str().unwrap().contains("The Daily Show"));

    // A window that excludes the date returns empty (not an error).
    let empty: Value = server
        .client()
        .get(server.url("/sonarr/api/v3/calendar?start=2026-08-01&end=2026-08-31"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(empty.as_array().unwrap().len(), 0);
}

// --- root folder CRUD ------------------------------------------------------

#[tokio::test]
async fn root_folder_create_list_delete_round_trips() {
    let server = start_authed().await;

    // Create a standalone root folder.
    let created: Value = server
        .client()
        .post(server.url("/api/v3/rootfolder"))
        .header("x-api-key", TEST_API_KEY)
        .json(&json!({ "path": "/data/extra-movies" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_i64().expect("numeric root-folder id");
    assert_eq!(created["path"], "/data/extra-movies");

    // It is listed.
    let list: Value = server
        .client()
        .get(server.url("/api/v3/rootfolder"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "/data/extra-movies"),
        "created root folder is listed: {list}"
    );

    // Delete it (idempotent 200).
    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/rootfolder/{id}")))
        .header("x-api-key", TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Gone from the list.
    let after: Value = server
        .client()
        .get(server.url("/api/v3/rootfolder"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !after
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "/data/extra-movies"),
        "deleted root folder is gone: {after}"
    );

    // A re-issued delete still returns 200 (idempotent).
    let again = server
        .client()
        .delete(server.url(&format!("/api/v3/rootfolder/{id}")))
        .header("x-api-key", TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 200);
}

/// Regression: a library that imports into the SAME path as a standalone root
/// folder must NOT shadow it with a phantom, undeletable library-derived entry.
/// The standalone folder (real id projection) is surfaced and DELETE removes it.
#[tokio::test]
async fn library_root_folder_does_not_shadow_standalone() {
    let server = start_authed().await;
    let path = "/data/shared-root";

    // A standalone root folder at `path`.
    let rf = cellarr_core::RootFolder {
        id: "shared-root-id".to_string(),
        path: path.to_string(),
        name: Some("shared".to_string()),
        enabled: true,
    };
    server
        .state
        .db
        .config()
        .upsert_root_folder(&rf)
        .await
        .unwrap();

    // A library that imports into the SAME path (library.root_folders hold paths).
    let profile = common::seed_profile(&server.state, "shadow-prof").await;
    let lib = cellarr_core::Library {
        id: cellarr_core::LibraryId::new(),
        media_type: MediaType::Movie,
        name: "Shadow".to_string(),
        root_folders: vec![path.to_string()],
        default_quality_profile: profile,
    };
    server.state.db.config().upsert_library(&lib).await.unwrap();

    // Exactly one root-folder entry for the path (no phantom duplicate).
    let list: Value = server
        .client()
        .get(server.url("/api/v3/rootfolder"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let matches: Vec<&Value> = list
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["path"] == path)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "one entry for the path, not a phantom dup: {list}"
    );
    let id = matches[0]["id"].as_i64().expect("numeric id");

    // The listed entry is the real standalone folder (real id projection), so
    // DELETE/{id} removes the standalone row. (A phantom library-derived entry
    // would carry a sequential index that resolves to nothing — the bug.) The
    // path may still appear afterward as a library-derived projection since the
    // library keeps referencing it; what matters is the standalone row is gone.
    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/rootfolder/{id}")))
        .header("x-api-key", TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let standalone_gone = !server
        .state
        .db
        .config()
        .list_root_folders()
        .await
        .unwrap()
        .iter()
        .any(|rf| rf.path == path);
    assert!(
        standalone_gone,
        "DELETE removed the real standalone root folder (it was not a phantom)"
    );
}

// --- interactive grab (no pipeline wired) ----------------------------------

#[tokio::test]
async fn grab_release_without_pipeline_is_not_grabbed_not_405() {
    let server = start_authed().await;
    let id = seed_movie(&server.state, "The Matrix", true).await;

    // The API's own state has no release_grab seam wired (offline default), so a
    // grab is reported clearly as not grabbed — crucially NOT a 405 (the bug this
    // route fixes: the FE's POST used to 405).
    let resp = server
        .client()
        .post(server.url("/api/v3/release"))
        .header("x-api-key", TEST_API_KEY)
        .json(&json!({ "guid": "guid-1080p", "contentId": id.to_string() }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "grab route exists (was 405 before)");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["guid"], "guid-1080p");
    assert_eq!(body["grabbed"], false);
    assert!(body["message"].is_string());
}

#[tokio::test]
async fn grab_release_requires_guid_and_content_id() {
    let server = start_authed().await;

    // Missing guid -> 400.
    let resp = server
        .client()
        .post(server.url("/api/v3/release"))
        .header("x-api-key", TEST_API_KEY)
        .json(&json!({ "contentId": ContentId::new().to_string() }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}
