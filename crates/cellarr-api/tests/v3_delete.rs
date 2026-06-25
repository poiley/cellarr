//! `/api/v3` content delete: `DELETE /movie/{id}` and `DELETE /series/{id}`,
//! with the `deleteFiles` and `addImportExclusion` flags (mirroring Radarr /
//! Sonarr). HERMETIC: the standard file-backed test server; the file-deleting
//! tests place real media under a temp library root and assert the bytes are
//! recycled (or unlinked) only when asked.

mod common;

use std::path::PathBuf;

use common::{seed_library, seed_library_rooted, start_open, start_with_recycle_bin};
use serde_json::Value;

use cellarr_core::importlist::ImportListRepository;
use cellarr_core::repo::{ContentRepository, MediaFileRepository};
use cellarr_core::{
    ContentId, ContentKind, ContentNode, Coordinates, MediaFile, MediaFileId, MediaType,
};

/// Seed a movie node (optionally with a media file on disk) and return its id.
async fn seed_movie(
    state: &cellarr_api::AppState,
    library: cellarr_core::LibraryId,
    title: &str,
) -> ContentId {
    let id = ContentId::new();
    let node = ContentNode {
        tags: Vec::new(),
        id,
        library_id: library,
        media_type: MediaType::Movie,
        parent_id: None,
        kind: ContentKind::Movie,
        coords: Coordinates::Movie,
        monitored: true,
        title_id: None,
    };
    state.db.content().upsert(&node).await.unwrap();
    state.db.content().index_title(id, title).await.unwrap();
    id
}

/// Link a media file at `path` (size from the real file on disk) to `content`.
async fn link_file(state: &cellarr_api::AppState, content: ContentId, path: &str) -> MediaFileId {
    let file = MediaFile {
        id: MediaFileId::new(),
        path: path.to_string(),
        size: 7,
        quality: cellarr_core::profile::Quality::new("Bluray-1080p", 14),
        languages: vec![],
        media_info: None,
        custom_format_score: None,
        release_type: None,
    };
    let fid = file.id;
    state.db.media_files().create(&file).await.unwrap();
    state.db.media_files().link(content, fid).await.unwrap();
    fid
}

#[tokio::test]
async fn delete_movie_removes_record_returns_200() {
    let server = start_open().await;
    let library = seed_library(&server.state, MediaType::Movie, "Movies").await;
    let id = seed_movie(&server.state, library, "The Matrix").await;

    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/movie/{id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // The record is gone.
    assert!(server.state.db.content().get(id).await.unwrap().is_none());
}

#[tokio::test]
async fn delete_movie_is_idempotent_on_missing_id() {
    let server = start_open().await;
    // Never-existed id still returns 200 (the *arr re-issue-delete contract).
    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/movie/{}", ContentId::new())))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn delete_files_false_leaves_files_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("movies");
    std::fs::create_dir_all(&root).unwrap();
    let media = root.join("The Matrix (1999)/movie.mkv");
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
    let id = seed_movie(&server.state, library, "The Matrix").await;
    link_file(&server.state, id, media.to_str().unwrap()).await;

    // Default (deleteFiles unset) removes the record but leaves the file.
    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/movie/{id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(server.state.db.content().get(id).await.unwrap().is_none());
    assert!(
        media.exists(),
        "deleteFiles=false must leave the file on disk"
    );
}

#[tokio::test]
async fn delete_files_true_unlinks_when_no_recycle_bin() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("movies");
    let media = root.join("Heat (1995)/movie.mkv");
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
    let id = seed_movie(&server.state, library, "Heat").await;
    link_file(&server.state, id, media.to_str().unwrap()).await;

    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/movie/{id}?deleteFiles=true")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(!media.exists(), "deleteFiles=true with no bin must unlink");
}

#[tokio::test]
async fn delete_files_true_recycles_into_bin() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("movies");
    let bin = dir.path().join("recycle");
    let media = root.join("Drive (2011)/movie.mkv");
    std::fs::create_dir_all(media.parent().unwrap()).unwrap();
    std::fs::write(&media, b"payload").unwrap();

    let server = start_with_recycle_bin(bin.clone()).await;
    let library = seed_library_rooted(
        &server.state,
        MediaType::Movie,
        "Movies",
        root.to_str().unwrap(),
    )
    .await;
    let id = seed_movie(&server.state, library, "Drive").await;
    link_file(&server.state, id, media.to_str().unwrap()).await;

    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/movie/{id}?deleteFiles=true")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    // Original gone, copy preserved in the bin under the same relative layout.
    assert!(!media.exists(), "the original must be moved out");
    let recycled = bin.join("Drive (2011)/movie.mkv");
    assert!(recycled.exists(), "the file must be recycled into the bin");
    assert_eq!(std::fs::read(&recycled).unwrap(), b"payload");
}

#[tokio::test]
async fn add_import_exclusion_records_exclusion() {
    let server = start_open().await;
    let library = seed_library(&server.state, MediaType::Movie, "Movies").await;
    let id = seed_movie(&server.state, library, "Blade Runner").await;

    // No exclusions before.
    assert!(server
        .state
        .db
        .import_lists()
        .list_exclusions()
        .await
        .unwrap()
        .is_empty());

    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/movie/{id}?addImportExclusion=true")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let exclusions = server
        .state
        .db
        .import_lists()
        .list_exclusions()
        .await
        .unwrap();
    assert_eq!(exclusions.len(), 1, "an exclusion must be recorded");
    assert_eq!(exclusions[0].id_value, id.to_string());
    assert_eq!(exclusions[0].title, "Blade Runner");
}

#[tokio::test]
async fn delete_series_removes_subtree_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tv");
    let media = root.join("Show/Season 01/S01E01.mkv");
    std::fs::create_dir_all(media.parent().unwrap()).unwrap();
    std::fs::write(&media, b"episode").unwrap();

    let server = start_open().await;
    let library =
        seed_library_rooted(&server.state, MediaType::Tv, "TV", root.to_str().unwrap()).await;

    // series -> season -> episode (with a file).
    let series = ContentId::new();
    let content = server.state.db.content();
    content
        .upsert(&ContentNode {
            tags: Vec::new(),
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
            tags: Vec::new(),
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
    let episode = ContentId::new();
    content
        .upsert(&ContentNode {
            tags: Vec::new(),
            id: episode,
            library_id: library,
            media_type: MediaType::Tv,
            parent_id: Some(season),
            kind: ContentKind::Episode,
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
    link_file(&server.state, episode, media.to_str().unwrap()).await;

    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/series/{series}?deleteFiles=true")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // The whole subtree is gone and the file removed.
    for n in [series, season, episode] {
        assert!(content.get(n).await.unwrap().is_none());
    }
    assert!(!media.exists(), "the episode file must be removed");
}

#[tokio::test]
async fn delete_series_does_not_touch_files_outside_root() {
    // A media_file whose stored path escapes the library root must NOT be deleted:
    // the file step refuses the escape, so the on-disk file survives even with
    // deleteFiles=true. (The DB record is still removed — that is in-DB only.)
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tv");
    std::fs::create_dir_all(&root).unwrap();
    let outside = dir.path().join("outside/secret.mkv");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::write(&outside, b"do not touch").unwrap();
    // Reference it via a traversal rooted at the library.
    let escaping = PathBuf::from(format!("{}/../outside/secret.mkv", root.to_str().unwrap()));

    let server = start_open().await;
    let library =
        seed_library_rooted(&server.state, MediaType::Tv, "TV", root.to_str().unwrap()).await;
    let id = seed_movie_as_series(&server.state, library, "Bad Show").await;
    link_file(&server.state, id, escaping.to_str().unwrap()).await;

    let resp = server
        .client()
        .delete(server.url(&format!("/api/v3/series/{id}?deleteFiles=true")))
        .send()
        .await
        .unwrap();
    // The file step refuses the escape → the endpoint reports an error, and the
    // out-of-root file is untouched.
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    assert!(
        status.is_server_error() || status.is_client_error(),
        "an escaping delete must not 200: got {status} {body}"
    );
    assert!(
        outside.exists(),
        "a file outside the root must never be deleted"
    );
}

/// Seed a series root node (used by the path-escape test).
async fn seed_movie_as_series(
    state: &cellarr_api::AppState,
    library: cellarr_core::LibraryId,
    title: &str,
) -> ContentId {
    let id = ContentId::new();
    state
        .db
        .content()
        .upsert(&ContentNode {
            tags: Vec::new(),
            id,
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
    state.db.content().index_title(id, title).await.unwrap();
    id
}
