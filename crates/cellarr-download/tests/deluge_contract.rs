//! Deluge JSON-RPC contract tests (record/replay; no live client).
//!
//! Pins the cookie login, magnet/file add through the Label plugin, the
//! status->lifecycle mapping (downloading -> seeding with content_path), the
//! ratio-gated remove, and the auth/not-found/errored/foreign-label edges — the
//! same lifecycle contract the qBittorrent/Transmission tests assert.
//!
//! Live validation against a real Deluge daemon is DEFERRED (no server available
//! in this environment); the record/replay seam is the standing substitute.

mod common;

use cellarr_core::DownloadState;
use cellarr_download::{DelugeClient, DelugeSettings, DownloadError, HttpTransport, RemovePolicy};
use common::{torrent_grab, ReplayTransport};

fn settings_with_dir() -> DelugeSettings {
    DelugeSettings {
        base_url: Some("http://localhost:8112".into()),
        host: None,
        port: None,
        url_base: None,
        password: "secret".into(),
        download_dir: Some("/downloads".into()),
    }
}

fn settings_no_dir() -> DelugeSettings {
    DelugeSettings {
        base_url: Some("http://localhost:8112".into()),
        host: None,
        port: None,
        url_base: None,
        password: "secret".into(),
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
    settings: DelugeSettings,
) -> (DelugeClient, std::sync::Arc<ReplayTransport>) {
    let transport = std::sync::Arc::new(ReplayTransport::load(fixture));
    let client = DelugeClient::with_transport(
        "deluge",
        settings,
        category,
        Box::new(ArcTransport(transport.clone())),
    );
    (client, transport)
}

#[tokio::test]
async fn full_lifecycle_cookie_login_label_and_ratio_gated_remove() {
    let (client, transport) =
        client_with("deluge/lifecycle.json", "cellarr-tv", settings_with_dir());
    let grab = torrent_grab(
        "magnet:?xt=urn:btih:deadbeefcafef00d&dn=Show.S01E01",
        "cellarr-tv",
    );

    let id = client.add(&grab).await.expect("add");
    assert_eq!(id, "deadbeefcafef00d");

    // Still downloading: no content_path yet, label scopes it.
    let p = client.progress(&id).await.expect("progress downloading");
    assert_eq!(p.state, DownloadState::Downloading);
    assert!(p.content_path.is_none());
    assert!(p.is_in_category("cellarr-tv"));

    // Seeding/completed but ratio 1.5 < 2.0 and time 7200 < 86400: not removable.
    let policy = RemovePolicy {
        min_ratio: Some(2.0),
        min_seeding_time_secs: Some(86_400),
        delete_data: true,
    };
    let removed = client.remove(&id, policy).await.expect("remove attempt 1");
    assert!(!removed, "should not remove before ratio/time gate met");

    // Ratio now 2.5 >= 2.0: removable. content_path = download_location/name.
    let removed = client.remove(&id, policy).await.expect("remove attempt 2");
    assert!(removed, "should remove once ratio gate satisfied");

    transport.assert_drained();
}

#[tokio::test]
async fn completed_status_projects_content_path_to_core() {
    let (client, transport) =
        client_with("deluge/lifecycle.json", "cellarr-tv", settings_with_dir());
    let grab = torrent_grab(
        "magnet:?xt=urn:btih:deadbeefcafef00d&dn=Show.S01E01",
        "cellarr-tv",
    );
    let _ = client.add(&grab).await.expect("add");
    let _ = client
        .progress("deadbeefcafef00d")
        .await
        .expect("downloading");

    let p = client.progress("deadbeefcafef00d").await.expect("seeding");
    assert_eq!(p.state, DownloadState::Completed);
    let core = p.to_core_status();
    assert!(core.is_completed());
    assert_eq!(core.content_path.as_deref(), Some("/downloads/Show.S01E01"));
    assert_eq!(core.ratio, Some(1.5));
    assert_eq!(core.seeding_time_secs, Some(7200));

    // Drain the fixture with the final ratio-gated remove.
    client
        .remove("deadbeefcafef00d", RemovePolicy::immediate(true))
        .await
        .expect("remove");
    transport.assert_drained();
}

#[tokio::test]
async fn add_with_http_url_fetches_torrent_and_sends_base64_file() {
    let (client, transport) = client_with(
        "deluge/add_http_torrent_file.json",
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
async fn login_result_false_maps_to_auth_error() {
    let (client, _t) = client_with("deluge/login_failure.json", "cellarr-tv", settings_no_dir());
    let err = client.status("whatever").await.unwrap_err();
    assert!(matches!(err, DownloadError::Auth(_)), "got {err:?}");
}

#[tokio::test]
async fn empty_status_object_maps_to_not_found() {
    let (client, transport) = client_with(
        "deluge/status_not_found.json",
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
        "deluge/status_errored_and_foreign.json",
        "cellarr-tv",
        settings_no_dir(),
    );

    let p = client.progress("erroredhash").await.expect("errored");
    assert_eq!(p.state, DownloadState::Failed);
    assert_eq!(p.error_string.as_deref(), Some("Tracker gave error 410"));

    let p = client.progress("foreignhash").await.expect("foreign");
    assert_eq!(p.state, DownloadState::Completed);
    assert!(!p.is_in_category("cellarr-tv"));
    assert!(p.is_in_category("manual-stuff"));
    assert_eq!(p.content_path.as_deref(), Some("/other/thing"));
    assert_eq!(p.ratio, Some(3.0));
    assert_eq!(p.seeding_time_secs, Some(100_000));

    transport.assert_drained();
}
