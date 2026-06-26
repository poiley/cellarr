//! `/api/v3` compatibility resources added for v3 ecosystem tools that call the
//! Sonarr/Radarr `parse`, `episodefile`/`moviefile`, `collection`, `metadata`, and
//! `update` endpoints (Overseerr, mobile apps, dashboards). HERMETIC: the standard
//! file-backed test server; the file-deleting tests place real media under a temp
//! library root and assert the bytes are removed via the crash-safe recycle path.

mod common;

use common::{seed_library, seed_library_rooted, start_open};
use serde_json::Value;

use cellarr_core::importlist::ImportListRepository;
use cellarr_core::repo::{ContentRepository, MediaFileRepository};
use cellarr_core::{
    ContentId, ContentKind, ContentNode, Coordinates, ImportListConfig, MediaFile, MediaFileId,
    MediaType,
};

// --- /parse ----------------------------------------------------------------

#[tokio::test]
async fn parse_movie_title_returns_parsed_movie_info() {
    let server = start_open().await;
    seed_library(&server.state, MediaType::Movie, "Movies").await;

    let title = "Blade Runner 2049 2017 1080p BluRay x264-SPARKS";
    let resp = server
        .client()
        .get(server.url(&format!("/api/v3/parse?title={}", urlencoding(title))))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    assert_eq!(body["title"], title);
    let info = &body["parsedMovieInfo"];
    assert!(
        info.is_object(),
        "movie surface must yield parsedMovieInfo: {body}"
    );
    assert_eq!(info["year"], 2017, "parsed year");
    assert_eq!(info["releaseGroup"], "SPARKS", "parsed release group");
    assert_eq!(
        info["quality"]["quality"]["resolution"], 1080,
        "parsed resolution pixels"
    );
    assert_eq!(
        info["quality"]["quality"]["source"], "bluray",
        "parsed source bucket"
    );
    assert_eq!(
        info["quality"]["quality"]["name"], "Bluray-1080p",
        "composed quality name"
    );
}

#[tokio::test]
async fn parse_episode_title_returns_parsed_episode_info() {
    let server = start_open().await;
    seed_library(&server.state, MediaType::Tv, "TV").await;

    let title = "The.Expanse.S03E05.1080p.WEB-DL.x264-GROUP";
    let resp = server
        .client()
        .get(server.url(&format!("/api/v3/parse?title={}", urlencoding(title))))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    let info = &body["parsedEpisodeInfo"];
    assert!(
        info.is_object(),
        "TV surface must yield parsedEpisodeInfo: {body}"
    );
    assert_eq!(info["seasonNumber"], 3, "parsed season");
    assert_eq!(
        info["episodeNumbers"],
        serde_json::json!([5]),
        "parsed episode"
    );
    assert_eq!(info["releaseGroup"], "GROUP", "parsed release group");
    assert_eq!(
        info["quality"]["quality"]["resolution"], 1080,
        "parsed resolution"
    );
}

#[tokio::test]
async fn parse_requires_a_title_or_path() {
    let server = start_open().await;
    let resp = server
        .client()
        .get(server.url("/api/v3/parse"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "no title and no path is a 400");
}

#[tokio::test]
async fn parse_falls_back_to_path_file_name() {
    let server = start_open().await;
    seed_library(&server.state, MediaType::Movie, "Movies").await;
    let path = "/downloads/Heat 1995 2160p BluRay x265-GRP/Heat 1995 2160p BluRay x265-GRP.mkv";
    let resp = server
        .client()
        .get(server.url(&format!("/api/v3/parse?path={}", urlencoding(path))))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["parsedMovieInfo"]["year"], 1995);
    assert_eq!(
        body["parsedMovieInfo"]["quality"]["quality"]["resolution"],
        2160
    );
}

// --- /moviefile + /episodefile ---------------------------------------------

/// Seed a movie node with a media file on disk, returning (content id, file id).
async fn seed_movie_with_file(
    state: &cellarr_api::AppState,
    library: cellarr_core::LibraryId,
    title: &str,
    path: &str,
) -> (ContentId, MediaFileId) {
    let id = ContentId::new();
    state
        .db
        .content()
        .upsert(&ContentNode {
            tags: Vec::new(),
            id,
            library_id: library,
            media_type: MediaType::Movie,
            parent_id: None,
            kind: ContentKind::Movie,
            series_type: cellarr_core::SeriesType::Standard,
            coords: Coordinates::Movie,
            monitored: true,
            title_id: None,
        })
        .await
        .unwrap();
    state.db.content().index_title(id, title).await.unwrap();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(7);
    let file = MediaFile {
        id: MediaFileId::new(),
        path: path.to_string(),
        size,
        quality: cellarr_core::profile::Quality::new("Bluray-1080p", 14),
        languages: vec!["en".to_string()],
        media_info: None,
        custom_format_score: Some(25),
        release_type: None,
    };
    let fid = file.id;
    state.db.media_files().create(&file).await.unwrap();
    state.db.media_files().link(id, fid).await.unwrap();
    (id, fid)
}

#[tokio::test]
async fn moviefile_list_get_and_shape() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("movies");
    let media = root.join("The Matrix (1999)/movie.mkv");
    std::fs::create_dir_all(media.parent().unwrap()).unwrap();
    std::fs::write(&media, b"payload-bytes").unwrap();

    let server = start_open().await;
    let library = seed_library_rooted(
        &server.state,
        MediaType::Movie,
        "Movies",
        root.to_str().unwrap(),
    )
    .await;
    let (content_id, _fid) = seed_movie_with_file(
        &server.state,
        library,
        "The Matrix",
        media.to_str().unwrap(),
    )
    .await;

    // List by movieId (the full uuid spelling cellarr's own face accepts).
    let resp = server
        .client()
        .get(server.url(&format!("/api/v3/moviefile?movieId={content_id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let list: Vec<Value> = resp.json().await.unwrap();
    assert_eq!(list.len(), 1, "one file for the movie");
    let f = &list[0];
    assert_eq!(f["path"], media.to_str().unwrap());
    assert_eq!(f["relativePath"], "The Matrix (1999)/movie.mkv");
    assert_eq!(f["size"], "payload-bytes".len());
    assert_eq!(f["quality"]["quality"]["name"], "Bluray-1080p");
    assert_eq!(f["customFormatScore"], 25);
    let file_id = f["id"].as_i64().unwrap();

    // GET /{id} returns the same file.
    let resp = server
        .client()
        .get(server.url(&format!("/api/v3/moviefile/{file_id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let one: Value = resp.json().await.unwrap();
    assert_eq!(one["id"], file_id);
    assert_eq!(one["path"], media.to_str().unwrap());
}

#[tokio::test]
async fn moviefile_delete_removes_file_via_real_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("movies");
    let media = root.join("Drive (2011)/movie.mkv");
    std::fs::create_dir_all(media.parent().unwrap()).unwrap();
    std::fs::write(&media, b"payload").unwrap();

    let server = start_open().await;
    let library = seed_library_rooted(
        &server.state,
        MediaType::Movie,
        "Movies",
        root.to_str().unwrap(),
    )
    .await;
    let (content_id, fid) =
        seed_movie_with_file(&server.state, library, "Drive", media.to_str().unwrap()).await;

    // Read the file's projected id back through the list endpoint (the same id the
    // ecosystem would hold), rather than recomputing the private projection.
    let listed: Vec<Value> = server
        .client()
        .get(server.url(&format!("/api/v3/moviefile?movieId={content_id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let file_id = listed[0]["id"].as_i64().unwrap();

    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/moviefile/{file_id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // The bytes are gone (no recycle bin configured -> unlinked via the real path)…
    assert!(!media.exists(), "delete must remove the file on disk");
    // …and the DB row is gone.
    assert!(server
        .state
        .db
        .media_files()
        .get(fid)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn moviefile_delete_is_idempotent_on_missing_id() {
    let server = start_open().await;
    let resp = server
        .client()
        .delete(server.url("/api/v3/moviefile/123456789"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "a missing file id still returns 200");
}

#[tokio::test]
async fn episodefile_list_walks_series_subtree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tv");
    let media = root.join("Show/Season 01/S01E01.mkv");
    std::fs::create_dir_all(media.parent().unwrap()).unwrap();
    std::fs::write(&media, b"episode").unwrap();

    let server = start_open().await;
    let library =
        seed_library_rooted(&server.state, MediaType::Tv, "TV", root.to_str().unwrap()).await;
    let content = server.state.db.content();

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
    content.index_title(series, "Show").await.unwrap();
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
    let episode = ContentId::new();
    content
        .upsert(&ContentNode {
            tags: Vec::new(),
            id: episode,
            library_id: library,
            media_type: MediaType::Tv,
            parent_id: Some(season),
            kind: ContentKind::Episode,
            series_type: cellarr_core::SeriesType::Standard,
            coords: Coordinates::Episode {
                season: 1,
                episode: 1,
                absolute: None,
            },
            monitored: true,
            title_id: None,
        })
        .await
        .unwrap();
    // The file links to the inner episode node.
    let file = MediaFile {
        id: MediaFileId::new(),
        path: media.to_str().unwrap().to_string(),
        size: 7,
        quality: cellarr_core::profile::Quality::new("WEBDL-1080p", 12),
        languages: vec![],
        media_info: None,
        custom_format_score: None,
        release_type: None,
    };
    let fid = file.id;
    server.state.db.media_files().create(&file).await.unwrap();
    server
        .state
        .db
        .media_files()
        .link(episode, fid)
        .await
        .unwrap();

    // Listing by the series id finds the file linked deep in the subtree, and it
    // reports the SERIES id as seriesId (not the inner episode node).
    let resp = server
        .client()
        .get(server.url(&format!("/api/v3/episodefile?seriesId={series}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let list: Vec<Value> = resp.json().await.unwrap();
    assert_eq!(
        list.len(),
        1,
        "the deep episode file is found via the subtree"
    );
    assert_eq!(list[0]["quality"]["quality"]["name"], "WEBDL-1080p");
    let series_id = list[0]["seriesId"].as_i64().unwrap();
    assert!(
        series_id != 0,
        "the file reports a non-zero owning series id"
    );
    assert_eq!(
        list[0]["relativePath"], "Show/Season 01/S01E01.mkv",
        "relativePath is computed against the library root"
    );

    // GET /{id} reports the same owning series id (the series root, not the inner
    // episode node the file links to).
    let file_id = list[0]["id"].as_i64().unwrap();
    let one: Value = server
        .client()
        .get(server.url(&format!("/api/v3/episodefile/{file_id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(one["seriesId"], series_id, "the owning series id is stable");
}

// --- /collection -----------------------------------------------------------

/// Seed a TMDb-collection import list (the existing collection seam).
async fn seed_collection_list(
    state: &cellarr_api::AppState,
    name: &str,
    collection_id: &str,
    monitored: bool,
) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let cfg = ImportListConfig {
        id: id.clone(),
        name: name.to_string(),
        kind: "collection".to_string(),
        enabled: true,
        media_type: MediaType::Movie,
        monitored,
        clean_action: cellarr_core::CleanAction::None,
        quality_profile_id: None,
        last_successful_sync: None,
        settings: serde_json::json!({ "collection_id": collection_id }),
    };
    state.db.import_lists().upsert(&cfg).await.unwrap();
    id
}

#[tokio::test]
async fn collection_list_get_and_monitor_put_persists() {
    let server = start_open().await;
    seed_library(&server.state, MediaType::Movie, "Movies").await;
    // A non-collection import list must NOT show up as a collection.
    let other = ImportListConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name: "Popular".to_string(),
        kind: "tmdb".to_string(),
        enabled: true,
        media_type: MediaType::Movie,
        monitored: true,
        clean_action: cellarr_core::CleanAction::None,
        quality_profile_id: None,
        last_successful_sync: None,
        settings: serde_json::json!({ "feed": "popular" }),
    };
    server.state.db.import_lists().upsert(&other).await.unwrap();
    let list_id =
        seed_collection_list(&server.state, "John Wick Collection", "404609", false).await;

    // List: exactly the one collection.
    let resp = server
        .client()
        .get(server.url("/api/v3/collection"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let list: Vec<Value> = resp.json().await.unwrap();
    assert_eq!(
        list.len(),
        1,
        "only the collection import list is a collection"
    );
    let c = &list[0];
    assert_eq!(c["title"], "John Wick Collection");
    assert_eq!(c["tmdbId"], 404609);
    assert_eq!(c["monitored"], false);
    let coll_id = c["id"].as_i64().unwrap();

    // GET /{id}.
    let resp = server
        .client()
        .get(server.url(&format!("/api/v3/collection/{coll_id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let one: Value = resp.json().await.unwrap();
    assert_eq!(one["id"], coll_id);

    // PUT toggles monitored true, and it persists onto the backing import list.
    let resp = server
        .client()
        .put(server.url(&format!("/api/v3/collection/{coll_id}")))
        .json(&serde_json::json!({ "monitored": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let updated: Value = resp.json().await.unwrap();
    assert_eq!(updated["monitored"], true);

    let persisted = server
        .state
        .db
        .import_lists()
        .get(&list_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        persisted.monitored,
        "monitor toggle must persist on the import list"
    );
}

#[tokio::test]
async fn collection_is_empty_on_sonarr_face() {
    let server = start_open().await;
    seed_collection_list(&server.state, "A Collection", "1", true).await;
    let resp = server
        .client()
        .get(server.url("/sonarr/api/v3/collection"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let list: Vec<Value> = resp.json().await.unwrap();
    assert!(list.is_empty(), "collections are Radarr-only");
}

// --- /metadata -------------------------------------------------------------

#[tokio::test]
async fn metadata_reflects_and_toggles_nfo_setting() {
    let server = start_open().await;

    // Default: the nfo consumer is enabled (write_nfo defaults true).
    let resp = server
        .client()
        .get(server.url("/api/v3/metadata"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let list: Vec<Value> = resp.json().await.unwrap();
    assert_eq!(list.len(), 1, "one metadata consumer (the nfo export)");
    assert_eq!(list[0]["enable"], true, "nfo export is on by default");
    let meta_id = list[0]["id"].as_i64().unwrap();

    // Disable it.
    let resp = server
        .client()
        .put(server.url(&format!("/api/v3/metadata/{meta_id}")))
        .json(&serde_json::json!({ "enable": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let updated: Value = resp.json().await.unwrap();
    assert_eq!(updated["enable"], false);

    // The setting persisted, and a fresh GET reflects it.
    let mm = server
        .state
        .db
        .config()
        .get_media_management()
        .await
        .unwrap();
    assert!(
        !mm.write_nfo,
        "the nfo toggle must persist on media management"
    );
    let resp = server
        .client()
        .get(server.url("/api/v3/metadata/1"))
        .send()
        .await
        .unwrap();
    let one: Value = resp.json().await.unwrap();
    assert_eq!(one["enable"], false, "GET reflects the disabled state");
}

// --- /update ---------------------------------------------------------------

#[tokio::test]
async fn update_returns_empty_stub() {
    let server = start_open().await;
    for path in ["/api/v3/update", "/api/v3/system/update"] {
        let resp = server.client().get(server.url(path)).send().await.unwrap();
        assert_eq!(resp.status(), 200, "{path} must be a valid 200, not a 404");
        let list: Vec<Value> = resp.json().await.unwrap();
        assert!(list.is_empty(), "{path} is an empty stub (no auto-update)");
    }
}

/// Minimal percent-encoder for the query strings used here (spaces and a few
/// reserved chars); the titles are otherwise URL-safe ASCII.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
