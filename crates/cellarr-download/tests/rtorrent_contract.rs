//! rTorrent XML-RPC contract tests (record/replay; no live client).
//!
//! Pins load.start/load.raw_start add (with the d.custom1 label + d.directory),
//! the d.multicall2 status->lifecycle mapping (downloading -> complete with
//! base_path content_path and permille ratio), the ratio-gated d.erase, the
//! HTTP-Basic 401 edge, and the not-found/errored/foreign-label edges.
//!
//! Live validation against a real rTorrent daemon is DEFERRED (no server available
//! in this environment); the record/replay seam over [`HttpTransport`] is the
//! standing substitute, identical to the qBittorrent/Transmission contracts.

mod common;

use cellarr_core::DownloadState;
use cellarr_download::{
    DownloadError, HttpTransport, RemovePolicy, RtorrentClient, RtorrentSettings,
};
use common::{torrent_grab, ReplayTransport};

fn settings_with_dir() -> RtorrentSettings {
    RtorrentSettings {
        base_url: Some("http://localhost:8080".into()),
        host: None,
        port: None,
        url_base: None,
        username: None,
        password: None,
        download_dir: Some("/downloads".into()),
    }
}

fn settings_no_dir() -> RtorrentSettings {
    RtorrentSettings {
        base_url: Some("http://localhost:8080".into()),
        host: None,
        port: None,
        url_base: None,
        username: None,
        password: None,
        download_dir: None,
    }
}

fn settings_with_auth() -> RtorrentSettings {
    RtorrentSettings {
        base_url: Some("http://localhost:8080".into()),
        host: None,
        port: None,
        url_base: None,
        username: Some("user".into()),
        password: Some("pw".into()),
        download_dir: None,
    }
}

/// A transport wrapper so the test keeps an `Arc` to call `assert_drained` while
/// the adapter owns a `Box<dyn HttpTransport>`.
struct ArcTransport(std::sync::Arc<ReplayTransport>);

#[async_trait::async_trait]
impl HttpTransport for ArcTransport {
    async fn send(
        &self,
        req: cellarr_download::HttpRequest,
    ) -> Result<cellarr_download::HttpResponse, DownloadError> {
        self.0.send(req).await
    }
}

fn client_with(
    fixture: &str,
    category: &str,
    settings: RtorrentSettings,
) -> (RtorrentClient, std::sync::Arc<ReplayTransport>) {
    let transport = std::sync::Arc::new(ReplayTransport::load(fixture));
    let client = RtorrentClient::with_transport(
        "rtorrent",
        settings,
        category,
        Box::new(ArcTransport(transport.clone())),
    );
    (client, transport)
}

#[tokio::test]
async fn full_lifecycle_load_label_and_ratio_gated_erase() {
    let (client, transport) =
        client_with("rtorrent/lifecycle.json", "cellarr-tv", settings_with_dir());
    let grab = torrent_grab(
        "magnet:?xt=urn:btih:deadbeefcafef00d&dn=Show.S01E01",
        "cellarr-tv",
    );

    let id = client.add(&grab).await.expect("add");
    assert_eq!(id, "deadbeefcafef00d");

    // Downloading: active, not complete -> in flight, no content_path.
    let p = client.progress(&id).await.expect("progress downloading");
    assert_eq!(p.state, DownloadState::Downloading);
    assert!(p.content_path.is_none());
    assert!(p.is_in_category("cellarr-tv"));

    // Complete but ratio 1.5 < 2.0: not removable. (rTorrent reports no seed-time
    // column in this set, so removal gates on ratio.)
    let policy = RemovePolicy {
        min_ratio: Some(2.0),
        min_seeding_time_secs: None,
        delete_data: true,
    };
    let removed = client.remove(&id, policy).await.expect("remove attempt 1");
    assert!(!removed, "should not remove before ratio gate met");

    // Ratio now 2.5 >= 2.0: removable. content_path = d.base_path.
    let removed = client.remove(&id, policy).await.expect("remove attempt 2");
    assert!(removed, "should remove once ratio gate satisfied");

    transport.assert_drained();
}

#[tokio::test]
async fn completed_status_projects_base_path_content_path_to_core() {
    let (client, transport) =
        client_with("rtorrent/lifecycle.json", "cellarr-tv", settings_with_dir());
    let grab = torrent_grab(
        "magnet:?xt=urn:btih:deadbeefcafef00d&dn=Show.S01E01",
        "cellarr-tv",
    );
    let _ = client.add(&grab).await.expect("add");
    let _ = client
        .progress("deadbeefcafef00d")
        .await
        .expect("downloading");

    let p = client.progress("deadbeefcafef00d").await.expect("complete");
    assert_eq!(p.state, DownloadState::Completed);
    let core = p.to_core_status();
    assert!(core.is_completed());
    assert_eq!(
        core.content_path.as_deref(),
        Some("/downloads/cellarr-tv/Show.S01E01")
    );
    assert_eq!(core.ratio, Some(1.5));

    client
        .remove("deadbeefcafef00d", RemovePolicy::immediate(true))
        .await
        .expect("remove");
    transport.assert_drained();
}

#[tokio::test]
async fn add_with_http_url_fetches_torrent_and_sends_raw_base64() {
    let (client, transport) = client_with(
        "rtorrent/add_http_torrent_raw.json",
        "cellarr-tv",
        settings_no_dir(),
    );
    let grab = torrent_grab(
        "http://127.0.0.1:19696/1/download?apikey=KEY&link=LINK",
        "cellarr-tv",
    );
    let id = client.add(&grab).await.expect("http add");
    assert_eq!(id, "157493ee02747f71737019e994e47f44e5f89b97");
    transport.assert_drained();
}

#[tokio::test]
async fn basic_auth_401_maps_to_auth_error() {
    let (client, transport) =
        client_with("rtorrent/auth_401.json", "cellarr-tv", settings_with_auth());
    let err = client.status("whatever").await.unwrap_err();
    assert!(matches!(err, DownloadError::Auth(_)), "got {err:?}");
    transport.assert_drained();
}

#[tokio::test]
async fn no_matching_row_maps_to_not_found() {
    let (client, transport) = client_with(
        "rtorrent/status_not_found.json",
        "cellarr-tv",
        settings_no_dir(),
    );
    let err = client.status("missing").await.unwrap_err();
    assert!(matches!(err, DownloadError::NotFound(_)), "got {err:?}");
    transport.assert_drained();
}

#[tokio::test]
async fn errored_torrent_is_failed_and_foreign_label_is_visible() {
    let (client, transport) = client_with(
        "rtorrent/status_errored_and_foreign.json",
        "cellarr-tv",
        settings_no_dir(),
    );

    let p = client.progress("erroredhash").await.expect("errored");
    assert_eq!(p.state, DownloadState::Failed);
    assert_eq!(
        p.error_string.as_deref(),
        Some("Download error: file too large")
    );

    let p = client.progress("foreignhash").await.expect("foreign");
    assert_eq!(p.state, DownloadState::Completed);
    assert!(!p.is_in_category("cellarr-tv"));
    assert!(p.is_in_category("manual-stuff"));
    assert_eq!(p.content_path.as_deref(), Some("/other/thing"));
    assert_eq!(p.ratio, Some(3.0));

    transport.assert_drained();
}
