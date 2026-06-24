//! qBittorrent contract tests (record/replay; no live client).
//!
//! Pins the full lifecycle plus the version-divergent 5.x login variants and the
//! auth/not-found/failed/foreign-category edges the spec requires.

mod common;

use cellarr_core::DownloadState;
use cellarr_download::{DownloadError, QbittorrentClient, QbittorrentSettings, RemovePolicy};
use common::{torrent_grab, ReplayTransport};

fn settings() -> QbittorrentSettings {
    QbittorrentSettings {
        base_url: "http://localhost:8080".into(),
        username: "admin".into(),
        password: "adminadmin".into(),
    }
}

fn client(fixture: &str, category: &str) -> (QbittorrentClient, std::sync::Arc<ReplayTransport>) {
    let transport = std::sync::Arc::new(ReplayTransport::load(fixture));
    let client = QbittorrentClient::with_transport(
        "qbit",
        settings(),
        category,
        Box::new(ArcTransport(transport.clone())),
    );
    (client, transport)
}

/// A transport wrapper so the test can keep an `Arc` to call `assert_drained`
/// while the adapter owns a `Box<dyn HttpTransport>`.
struct ArcTransport(std::sync::Arc<ReplayTransport>);

#[async_trait::async_trait]
impl cellarr_download::HttpTransport for ArcTransport {
    async fn send(
        &self,
        req: cellarr_download::HttpRequest,
    ) -> Result<cellarr_download::HttpResponse, DownloadError> {
        self.0.send(req).await
    }
}

#[tokio::test]
async fn full_lifecycle_with_legacy_login_and_ratio_gated_remove() {
    let (client, transport) = client("qbittorrent/lifecycle_legacy_login.json", "cellarr-tv");
    let grab = torrent_grab(
        "magnet:?xt=urn:btih:deadbeefcafef00d&dn=Show.S01E01",
        "cellarr-tv",
    );

    let id = client.add(&grab).await.expect("add");
    assert_eq!(id, "deadbeefcafef00d");

    // Still downloading.
    let p = client.progress(&id).await.expect("progress 1");
    assert_eq!(p.state, DownloadState::Downloading);
    assert_eq!(p.content_path.as_deref(), Some("/downloads/Show.S01E01"));
    assert!(p.is_in_category("cellarr-tv"));

    // Seeding/completed, but ratio 1.5 < target 2.0 and time 7200 < 86400: not
    // yet removable.
    let policy = RemovePolicy {
        min_ratio: Some(2.0),
        min_seeding_time_secs: Some(86_400),
        delete_data: true,
    };
    let removed = client.remove(&id, policy).await.expect("remove attempt 1");
    assert!(!removed, "should not remove before ratio/time gate met");

    // Ratio now 2.5 >= 2.0: removable.
    let removed = client.remove(&id, policy).await.expect("remove attempt 2");
    assert!(removed, "should remove once ratio gate satisfied");

    transport.assert_drained();
}

#[tokio::test]
async fn add_with_http_url_fetches_torrent_and_uploads_multipart_not_urls() {
    // The download_url is an http(s) Prowlarr proxy URL only cellarr can reach.
    // cellarr fetches it and uploads the .torrent as a multipart `torrents` file
    // part, so qBittorrent never tries (and fails) to fetch the indexer URL. The
    // returned id is the v1 infohash computed from the fetched metainfo's info dict.
    let (client, transport) = client("qbittorrent/add_http_torrent_multipart.json", "cellarr-tv");
    let grab = torrent_grab(
        "http://127.0.0.1:19696/1/download?apikey=KEY&link=LINK",
        "cellarr-tv",
    );
    let id = client.add(&grab).await.expect("http add");
    assert_eq!(id, "157493ee02747f71737019e994e47f44e5f89b97");
    transport.assert_drained();
}

#[tokio::test]
async fn add_with_http_url_redirecting_to_magnet_uses_urls_form() {
    // An http(s) download_url that redirects to a magnet resolves to a magnet, so
    // the add reverts to the simple urls= form (no fetch of indexer bytes needed).
    let (client, transport) = client("qbittorrent/add_http_redirect_to_magnet.json", "cellarr-tv");
    let grab = torrent_grab(
        "http://127.0.0.1:19696/1/download?apikey=KEY&link=LINK",
        "cellarr-tv",
    );
    let id = client.add(&grab).await.expect("http->magnet add");
    assert_eq!(id, "deadbeefcafef00d");
    transport.assert_drained();
}

#[tokio::test]
async fn accepts_5x_changed_login_body_via_sid_cookie() {
    let (client, transport) = client("qbittorrent/login_5x_changed_body.json", "cellarr-movies");
    // The 5.x build returns a non-`Ok.` body; the adapter must still authenticate
    // via the issued SID cookie and resend it on the next call.
    let state = QbittorrentClient::status(&client, "abc123")
        .await
        .expect("status");
    assert_eq!(state, DownloadState::Downloading);
    transport.assert_drained();
}

#[tokio::test]
async fn version_probe_and_set_category_send_sid_and_map_409() {
    let (client, transport) = client("qbittorrent/version_and_set_category.json", "cellarr-tv");

    let version = client.version().await.expect("version");
    assert_eq!(version, "v5.1.2");

    // A successful re-file.
    client
        .set_category("deadbeefcafef00d", "cellarr-tv")
        .await
        .expect("set_category ok");

    // A 409 (category does not exist) surfaces as a typed API error so the
    // caller can create the category first rather than silently failing.
    let err = client
        .set_category("deadbeefcafef00d", "cellarr-tv")
        .await
        .unwrap_err();
    assert!(matches!(err, DownloadError::Api(_)), "got {err:?}");

    transport.assert_drained();
}

#[tokio::test]
async fn login_fails_body_maps_to_auth_error() {
    let (client, _t) = client("qbittorrent/login_fails.json", "cellarr-tv");
    let err = client.status("whatever").await.unwrap_err();
    assert!(matches!(err, DownloadError::Auth(_)), "got {err:?}");
}

#[tokio::test]
async fn login_403_maps_to_auth_error() {
    let (client, _t) = client("qbittorrent/login_banned_403.json", "cellarr-tv");
    let err = client.status("whatever").await.unwrap_err();
    assert!(matches!(err, DownloadError::Auth(_)), "got {err:?}");
}

#[tokio::test]
async fn empty_info_array_maps_to_not_found() {
    let (client, transport) = client("qbittorrent/status_not_found.json", "cellarr-tv");
    let err = client.status("missing").await.unwrap_err();
    assert!(matches!(err, DownloadError::NotFound(_)), "got {err:?}");
    transport.assert_drained();
}

#[tokio::test]
async fn errored_torrent_is_failed_and_foreign_category_is_visible() {
    let (client, transport) = client("qbittorrent/status_errored_and_foreign.json", "cellarr-tv");

    let p = client
        .progress("erroredhash")
        .await
        .expect("errored progress");
    assert_eq!(p.state, DownloadState::Failed);

    // A foreign download surfaces its own category so the caller can refuse to
    // touch it (category scoping).
    let p = client
        .progress("foreignhash")
        .await
        .expect("foreign progress");
    assert_eq!(p.state, DownloadState::Completed);
    assert!(!p.is_in_category("cellarr-tv"));
    assert!(p.is_in_category("manual-stuff"));

    // The trait projection carries the same detail the executor needs: a
    // completed status exposes its on-disk content_path (required for Import) and
    // its seed signals for gated removal. Project from the already-fetched
    // progress so no extra (unrecorded) exchange is consumed.
    let core = p.to_core_status();
    assert!(core.is_completed());
    assert_eq!(core.content_path.as_deref(), Some("/other/thing"));
    assert_eq!(core.ratio, Some(3.0));
    assert_eq!(core.seeding_time_secs, Some(100_000));

    transport.assert_drained();
}
