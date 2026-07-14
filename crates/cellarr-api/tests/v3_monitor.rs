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
            tags: Vec::new(),
            id: series,
            library_id: library,
            media_type: MediaType::Tv,
            parent_id: None,
            kind: ContentKind::Series,
            series_type: cellarr_core::SeriesType::Standard,
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
            tags: Vec::new(),
            id: season,
            library_id: library,
            media_type: MediaType::Tv,
            parent_id: Some(series),
            kind: ContentKind::Season,
            series_type: cellarr_core::SeriesType::Standard,
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
                tags: Vec::new(),
                id,
                library_id: library,
                media_type: MediaType::Tv,
                parent_id: Some(season),
                kind: ContentKind::Episode,
                series_type: cellarr_core::SeriesType::Standard,
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
async fn list_episodes_returns_the_series_episode_set() {
    let server = start_open().await;
    let library = seed_library(&server.state, MediaType::Tv, "TV").await;
    let (series, _season, e1, e2) = seed_series_tree(&server.state, library).await;

    // Give e1 a real indexed title + a persisted air date, and unmonitor e2, so the
    // response carries real per-episode facts the monitor tree renders.
    let content = server.state.db.content();
    content.index_title(e1, "The Pilot").await.unwrap();
    content
        .set_metadata(
            e1,
            &cellarr_core::ContentMetadata {
                title: Some("The Pilot".into()),
                year: Some(2020),
                overview: None,
                runtime: None,
                air_date: Some("2020-04-01".into()),
                digital_date: None,
                genres: Vec::new(),
                rating: None,
                rating_votes: None,
            },
        )
        .await
        .unwrap();
    let mut e2_node = content.get_node(e2).await.unwrap().unwrap();
    e2_node.monitored = false;
    content.upsert(&e2_node).await.unwrap();

    let resp = server
        .client()
        .get(server.url(&format!("/api/v3/episode?seriesId={series}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let rows = body.as_array().expect("an array of episodes");
    assert_eq!(rows.len(), 2, "both episodes of the series are returned");

    // Ordered by season then episode.
    assert_eq!(rows[0]["seasonNumber"], 1);
    assert_eq!(rows[0]["episodeNumber"], 1);
    assert_eq!(rows[1]["episodeNumber"], 2);

    // e1 carries its identified facts; every row carries the parent series id.
    let row1 = &rows[0];
    assert_eq!(row1["title"], "The Pilot");
    assert_eq!(row1["monitored"], true);
    assert_eq!(row1["hasFile"], false, "no media file linked yet");
    assert_eq!(row1["airDate"], "2020-04-01");
    // A standard episode with no absolute number reports a null absoluteEpisodeNumber.
    assert_eq!(
        row1["absoluteEpisodeNumber"],
        serde_json::Value::Null,
        "a non-anime episode has no absolute number"
    );
    let series_numeric = row1["seriesId"].as_i64().expect("a numeric seriesId");
    assert_eq!(rows[1]["seriesId"].as_i64().unwrap(), series_numeric);

    // e2's monitor flag is reflected.
    assert_eq!(rows[1]["monitored"], false);

    // An unknown series id yields an empty array, never a 404.
    let resp = server
        .client()
        .get(server.url(&format!("/api/v3/episode?seriesId={}", ContentId::new())))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp
        .json::<serde_json::Value>()
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn list_episodes_surfaces_the_absolute_episode_number() {
    let server = start_open().await;
    let library = seed_library(&server.state, MediaType::Tv, "TV").await;
    let (series, _season, e1, _e2) = seed_series_tree(&server.state, library).await;

    // Reconcile a known absolute number onto e1 (the anime absolute→episode remap
    // populates `absolute` on the episode coordinate).
    let content = server.state.db.content();
    let mut e1_node = content.get_node(e1).await.unwrap().unwrap();
    e1_node.coords = Coordinates::Episode {
        season: 1,
        episode: 1,
        absolute: Some(13),
    };
    content.upsert(&e1_node).await.unwrap();

    let body: serde_json::Value = server
        .client()
        .get(server.url(&format!("/api/v3/episode?seriesId={series}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = body.as_array().expect("an array of episodes");
    let row1 = rows
        .iter()
        .find(|r| r["episodeNumber"] == 1)
        .expect("episode 1");
    assert_eq!(
        row1["absoluteEpisodeNumber"], 13,
        "the reconciled absolute number is surfaced"
    );
    // The other episode, with no absolute, still reports null.
    let row2 = rows
        .iter()
        .find(|r| r["episodeNumber"] == 2)
        .expect("episode 2");
    assert_eq!(row2["absoluteEpisodeNumber"], serde_json::Value::Null);
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

/// `seriesType` round-trips through the v3 series surface: a POST persists it,
/// the GET detail and list reflect it, and a PUT updates it — for each of the
/// three values (standard/daily/anime). An anime add is the switch that turns on
/// the absolute-numbering + scene-remap behavior, so it must survive the trip.
#[tokio::test]
async fn series_type_round_trips_through_v3_series() {
    let server = start_open().await;
    let _library = seed_library(&server.state, MediaType::Tv, "TV").await;

    // POST each series type and confirm the response echoes it back.
    for ty in ["standard", "daily", "anime"] {
        let resp = server
            .client()
            .post(server.url("/api/v3/series"))
            .json(&json!({ "title": format!("Show {ty}"), "seriesType": ty }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(
            body["seriesType"], ty,
            "POST /series must round-trip seriesType={ty}"
        );

        // The persisted node carries the type (read back through the repo).
        let id: cellarr_core::ContentId =
            cellarr_core::ContentId::from_uuid(body["id"].as_str().unwrap().parse().unwrap());
        let node = server
            .state
            .db
            .content()
            .get_node(id)
            .await
            .unwrap()
            .unwrap();
        let expected = match ty {
            "anime" => cellarr_core::SeriesType::Anime,
            "daily" => cellarr_core::SeriesType::Daily,
            _ => cellarr_core::SeriesType::Standard,
        };
        assert_eq!(node.series_type, expected, "node persists seriesType={ty}");

        // The GET detail surfaces the same type.
        let detail: serde_json::Value = server
            .client()
            .get(server.url(&format!("/api/v3/series/{id}")))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            detail["seriesType"], ty,
            "GET detail surfaces seriesType={ty}"
        );
    }

    // A series added as standard can be switched to anime via PUT — the runtime
    // way an operator turns on anime numbering for an existing show.
    let resp = server
        .client()
        .post(server.url("/api/v3/series"))
        .json(&json!({ "title": "Switch Me" }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["seriesType"], "standard", "defaults to standard");
    let id = body["id"].as_str().unwrap().to_string();

    let resp = server
        .client()
        .put(server.url(&format!("/api/v3/series/{id}")))
        .json(&json!({ "seriesType": "anime" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["seriesType"], "anime",
        "PUT updates seriesType to anime"
    );

    // A partial PUT that only flips monitored must NOT reset the series type.
    let resp = server
        .client()
        .put(server.url(&format!("/api/v3/series/{id}")))
        .json(&json!({ "monitored": false }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["monitored"], false);
    assert_eq!(
        body["seriesType"], "anime",
        "a monitored-only PUT must not reset seriesType"
    );
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
async fn manual_import_scan_without_a_folder_scans_roots_and_never_errors() {
    // `folder` is optional: omitting it scans the library roots for untracked
    // in-place files (orphans) instead of erroring. With no pipeline seam wired
    // (offline/test) it degrades to an empty array — never a 400/500.
    let server = start_open().await;
    let resp = server
        .client()
        .get(server.url("/api/v3/manualimport"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "no folder is not an error");
    let rows: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(rows, serde_json::json!([]), "degrades to an empty list");
}
