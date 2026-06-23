//! `/api/v3/importlist` (CRUD + schema + test) and `/api/v3/importlistexclusion`
//! across the faces, plus the iCal/ICS calendar feed
//! (`/feed/v3/calendar/{sonarr,radarr}.ics`).
//!
//! HERMETIC: the standard file-backed test server harness; no live services and
//! no credentials. The import-list CRUD round-trips through the real db; the
//! calendar feed is driven over the live router and asserted to be valid ICS with
//! VEVENTs.

mod common;

use common::{seed_library, start_authed, start_open, TEST_API_KEY};
use serde_json::{json, Value};

use cellarr_core::repo::ContentRepository;
use cellarr_core::{ContentId, ContentKind, ContentNode, Coordinates, MediaType};

// --- import list CRUD ------------------------------------------------------

#[tokio::test]
async fn import_list_schema_lists_credentialed_sources() {
    let server = start_open().await;
    for base in ["/api/v3", "/sonarr/api/v3", "/radarr/api/v3"] {
        let resp = server
            .client()
            .get(server.url(&format!("{base}/importlist/schema")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{base} schema");
        let body: Value = resp.json().await.unwrap();
        let arr = body.as_array().expect("schema is an array");
        assert!(!arr.is_empty(), "{base} schema must advertise sources");
        // The safe clean default is present on every template.
        for entry in arr {
            let fields = entry["fields"].as_array().unwrap();
            let clean = fields
                .iter()
                .find(|f| f["name"] == "cleanLibraryLevel")
                .expect("a cleanLibraryLevel field");
            assert_eq!(clean["value"], "disabled", "clean defaults to disabled");
        }
    }
}

#[tokio::test]
async fn import_list_crud_round_trips_and_defaults_clean_to_safe() {
    let server = start_authed().await;

    // Create a Trakt list on the Radarr face with NO clean action specified.
    let create_body = json!({
        "name": "My Watchlist",
        "implementation": "TraktList",
        "enabled": true,
        "shouldMonitor": true,
        "fields": [
            { "name": "client_id", "value": "abc" },
            { "name": "list", "value": "me/watchlist" }
        ]
    });
    let resp = server
        .client()
        .post(server.url("/radarr/api/v3/importlist"))
        .header("x-api-key", TEST_API_KEY)
        .json(&create_body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "create");
    let created: Value = resp.json().await.unwrap();
    let id = created["id"].as_i64().expect("numeric id");
    // The safe default: no clean action unless explicitly opted in.
    assert_eq!(created["cleanLibraryLevel"], "disabled");
    assert_eq!(created["lastSuccessfulSync"], Value::Null);
    assert_eq!(created["implementation"], "TraktList");

    // It is listed on the Radarr face.
    let list: Value = server
        .client()
        .get(server.url("/radarr/api/v3/importlist"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);

    // GET by id works.
    let one: Value = server
        .client()
        .get(server.url(&format!("/radarr/api/v3/importlist/{id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(one["name"], "My Watchlist");

    // Update it to opt into a destructive clean action.
    let update_body = json!({
        "name": "My Watchlist",
        "implementation": "TraktList",
        "enabled": true,
        "shouldMonitor": true,
        "cleanLibraryLevel": "removeAndKeep",
        "fields": [ { "name": "client_id", "value": "abc" } ]
    });
    let updated: Value = server
        .client()
        .put(server.url(&format!("/radarr/api/v3/importlist/{id}")))
        .header("x-api-key", TEST_API_KEY)
        .json(&update_body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["cleanLibraryLevel"], "removeAndKeep");

    // Delete it.
    let del = server
        .client()
        .delete(server.url(&format!("/radarr/api/v3/importlist/{id}")))
        .header("x-api-key", TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 200);
    let after: Value = server
        .client()
        .get(server.url("/radarr/api/v3/importlist"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(after.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn import_list_write_requires_api_key() {
    let server = start_authed().await;
    let resp = server
        .client()
        .post(server.url("/radarr/api/v3/importlist"))
        .json(&json!({ "name": "x", "implementation": "TraktList" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "write without key must be unauthorized");
}

#[tokio::test]
async fn import_list_exclusion_crud() {
    let server = start_authed().await;

    let resp = server
        .client()
        .post(server.url("/radarr/api/v3/importlistexclusion"))
        .header("x-api-key", TEST_API_KEY)
        .json(&json!({ "tmdbId": 603, "title": "The Matrix" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let created: Value = resp.json().await.unwrap();
    let id = created["id"].as_i64().unwrap();
    assert_eq!(created["tmdbId"], 603);

    let list: Value = server
        .client()
        .get(server.url("/radarr/api/v3/importlistexclusion"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);

    let del = server
        .client()
        .delete(server.url(&format!("/radarr/api/v3/importlistexclusion/{id}")))
        .header("x-api-key", TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 200);
}

// --- iCal calendar feed ----------------------------------------------------

/// Seed a TV daily-coded episode node carrying an air date, so the calendar feed
/// has a dated item to emit a VEVENT for.
async fn seed_dated_episode(state: &cellarr_api::AppState, title: &str, date: &str) {
    let library = seed_library(state, MediaType::Tv, "TV").await;
    let node = ContentNode {
        id: ContentId::new(),
        library_id: library,
        media_type: MediaType::Tv,
        parent_id: None,
        kind: ContentKind::Episode,
        coords: Coordinates::Daily {
            date: date.to_string(),
        },
        monitored: true,
        title_id: None,
    };
    state.db.content().upsert(&node).await.unwrap();
    state
        .db
        .content()
        .index_title(node.id, title)
        .await
        .unwrap();
}

#[tokio::test]
async fn sonarr_ics_feed_returns_valid_calendar_with_vevents() {
    let server = start_open().await;
    seed_dated_episode(&server.state, "The Daily Show", "2026-07-04").await;

    let resp = server
        .client()
        .get(server.url("/feed/v3/calendar/sonarr.ics"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(ct.contains("text/calendar"), "content-type was {ct}");

    let body = resp.text().await.unwrap();
    assert!(body.starts_with("BEGIN:VCALENDAR\r\n"));
    assert!(body.contains("VERSION:2.0"));
    assert!(body.trim_end().ends_with("END:VCALENDAR"));
    // A VEVENT for the dated episode, with the air date and a SUMMARY.
    assert_eq!(body.matches("BEGIN:VEVENT").count(), 1, "one VEVENT");
    assert!(body.contains("DTSTART;VALUE=DATE:20260704"));
    assert!(body.contains("SUMMARY:The Daily Show"));
}

#[tokio::test]
async fn radarr_ics_feed_is_valid_even_when_empty() {
    let server = start_open().await;
    let resp = server
        .client()
        .get(server.url("/feed/v3/calendar/radarr.ics"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("BEGIN:VCALENDAR"));
    assert!(body.contains("END:VCALENDAR"));
    // No items -> valid but empty calendar (never a crash, never garbage).
    assert_eq!(body.matches("BEGIN:VEVENT").count(), 0);
}

#[tokio::test]
async fn calendar_feed_enforces_apikey_query_auth() {
    let server = start_authed().await;

    // No apikey -> 401 (calendar clients can only authenticate via the query).
    let resp = server
        .client()
        .get(server.url("/feed/v3/calendar/sonarr.ics"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "feed without apikey must be unauthorized"
    );

    // Correct apikey in the query -> 200.
    let ok = server
        .client()
        .get(server.url(&format!(
            "/feed/v3/calendar/sonarr.ics?apikey={TEST_API_KEY}"
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200, "feed with correct apikey must succeed");
}

#[tokio::test]
async fn unknown_calendar_file_is_404() {
    let server = start_open().await;
    let resp = server
        .client()
        .get(server.url("/feed/v3/calendar/bogus.ics"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}
