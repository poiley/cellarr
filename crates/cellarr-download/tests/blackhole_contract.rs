//! Blackhole / watch-folder adapter contract test.
//!
//! Exercises the full add → track → complete → remove lifecycle entirely on the
//! filesystem (tempdirs), with no live download client and no network: `add`
//! drops the job into the watch dir; we *simulate the external client* by placing
//! a finished file in the completed dir; `status` then reports `Completed` with a
//! `content_path` Import can read. This is the universal-client guarantee — it
//! works regardless of which torrent/usenet tool watches the folder.

mod common;

use std::collections::BTreeMap;

use async_trait::async_trait;
use cellarr_core::release::Protocol;
use cellarr_core::{DownloadClient, DownloadState};
use cellarr_download::error::DownloadError;
use cellarr_download::http::{HttpRequest, HttpResponse, HttpTransport};
use cellarr_download::{BlackholeClient, BlackholeSettings};

/// A transport that never expects to be called: a magnet add must not fetch.
struct PanicTransport;

#[async_trait]
impl HttpTransport for PanicTransport {
    async fn send(&self, _req: HttpRequest) -> Result<HttpResponse, DownloadError> {
        panic!("blackhole magnet add must not touch the network");
    }
}

/// A transport that returns fixed bytes for a `.torrent`/`.nzb` fetch.
struct BytesTransport {
    body: String,
}

#[async_trait]
impl HttpTransport for BytesTransport {
    async fn send(&self, _req: HttpRequest) -> Result<HttpResponse, DownloadError> {
        Ok(HttpResponse {
            status: 200,
            headers: BTreeMap::new(),
            body: self.body.clone(),
        })
    }
}

fn settings(watch: &std::path::Path, completed: &std::path::Path) -> BlackholeSettings {
    BlackholeSettings {
        watch_folder: watch.to_string_lossy().into_owned(),
        completed_folder: completed.to_string_lossy().into_owned(),
    }
}

#[tokio::test]
async fn magnet_add_writes_magnet_file_without_network() {
    let dir = tempfile::tempdir().unwrap();
    let watch = dir.path().join("watch");
    let completed = dir.path().join("completed");
    let client = BlackholeClient::with_transport(
        "blackhole",
        settings(&watch, &completed),
        "cellarr",
        Protocol::Torrent,
        Box::new(PanicTransport),
    );

    let grab = common::torrent_grab("magnet:?xt=urn:btih:abc123", "cellarr");
    let id = DownloadClient::add(&client, &grab).await.unwrap();

    // The id is the sanitized title stem; the magnet was written verbatim.
    let written = watch.join(format!("{id}.magnet"));
    assert!(written.exists(), "magnet file should be in watch dir");
    let body = std::fs::read_to_string(&written).unwrap();
    assert_eq!(body, "magnet:?xt=urn:btih:abc123");
}

#[tokio::test]
async fn torrent_url_add_fetches_and_writes_torrent() {
    let dir = tempfile::tempdir().unwrap();
    let watch = dir.path().join("watch");
    let completed = dir.path().join("completed");
    let client = BlackholeClient::with_transport(
        "blackhole",
        settings(&watch, &completed),
        "cellarr",
        Protocol::Torrent,
        Box::new(BytesTransport {
            body: "d8:announce..e".into(),
        }),
    );

    let grab = common::torrent_grab("http://indexer.test/x.torrent", "cellarr");
    let id = DownloadClient::add(&client, &grab).await.unwrap();

    let written = watch.join(format!("{id}.torrent"));
    assert!(written.exists());
    assert_eq!(std::fs::read_to_string(&written).unwrap(), "d8:announce..e");
}

#[tokio::test]
async fn status_is_downloading_until_completed_file_appears() {
    let dir = tempfile::tempdir().unwrap();
    let watch = dir.path().join("watch");
    let completed = dir.path().join("completed");
    let client = BlackholeClient::with_transport(
        "blackhole",
        settings(&watch, &completed),
        "cellarr",
        Protocol::Usenet,
        Box::new(PanicTransport),
    );

    // Add a usenet (nzb) job via a magnet-free path: write a fake nzb URL fetch.
    let client_fetch = BlackholeClient::with_transport(
        "blackhole",
        settings(&watch, &completed),
        "cellarr",
        Protocol::Usenet,
        Box::new(BytesTransport {
            body: "<nzb/>".into(),
        }),
    );
    let grab = common::usenet_grab("http://indexer.test/x.nzb", "cellarr");
    let id = DownloadClient::add(&client_fetch, &grab).await.unwrap();

    // Before the external tool finishes: Downloading.
    let status = DownloadClient::status(&client, &id).await.unwrap();
    assert_eq!(status.state, DownloadState::Downloading);
    assert!(status.content_path.is_none());

    // Simulate the external usenet tool dropping a finished file in completed.
    std::fs::create_dir_all(&completed).unwrap();
    let finished = completed.join(format!("{id}.mkv"));
    std::fs::write(&finished, b"video").unwrap();

    // Now: Completed, content_path points at the finished file.
    let status = DownloadClient::status(&client, &id).await.unwrap();
    assert_eq!(status.state, DownloadState::Completed);
    assert_eq!(
        status.content_path.as_deref(),
        Some(finished.to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn status_matches_a_completed_folder_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let watch = dir.path().join("watch");
    let completed = dir.path().join("completed");
    let client = BlackholeClient::with_transport(
        "blackhole",
        settings(&watch, &completed),
        "cellarr",
        Protocol::Torrent,
        Box::new(PanicTransport),
    );

    let grab = common::torrent_grab("magnet:?xt=urn:btih:folder", "cellarr");
    let id = DownloadClient::add(&client, &grab).await.unwrap();

    // The external client drops a finished *folder* named after the job.
    let folder = completed.join(&id);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("ep.mkv"), b"video").unwrap();

    let status = DownloadClient::status(&client, &id).await.unwrap();
    assert_eq!(status.state, DownloadState::Completed);
    assert_eq!(
        status.content_path.as_deref(),
        Some(folder.to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn unknown_id_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let client = BlackholeClient::with_transport(
        "blackhole",
        settings(&dir.path().join("watch"), &dir.path().join("completed")),
        "cellarr",
        Protocol::Torrent,
        Box::new(PanicTransport),
    );
    let err = DownloadClient::status(&client, "nope").await.unwrap_err();
    assert!(matches!(err, DownloadError::NotFound(_)));
}

#[tokio::test]
async fn remove_clears_watch_artifact_and_optionally_data() {
    let dir = tempfile::tempdir().unwrap();
    let watch = dir.path().join("watch");
    let completed = dir.path().join("completed");
    let client = BlackholeClient::with_transport(
        "blackhole",
        settings(&watch, &completed),
        "cellarr",
        Protocol::Torrent,
        Box::new(PanicTransport),
    );

    let grab = common::torrent_grab("magnet:?xt=urn:btih:rm", "cellarr");
    let id = DownloadClient::add(&client, &grab).await.unwrap();
    std::fs::create_dir_all(&completed).unwrap();
    let finished = completed.join(format!("{id}.mkv"));
    std::fs::write(&finished, b"video").unwrap();

    // remove(delete_data=false): drops the watch job, keeps the finished data.
    DownloadClient::remove(&client, &id, false).await.unwrap();
    assert!(!watch.join(format!("{id}.magnet")).exists());
    assert!(finished.exists());

    // remove(delete_data=true): also clears the finished data. Idempotent.
    DownloadClient::remove(&client, &id, true).await.unwrap();
    assert!(!finished.exists());
    DownloadClient::remove(&client, &id, true).await.unwrap();
}
