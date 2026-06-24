//! Transmission RPC contract tests (record/replay; no live client).
//!
//! Pins the CSRF `409` session handshake (capture + resend, plus mid-session
//! rotation), `torrent-add` (magnet, paused, label + download-dir), `torrent-get`
//! lifecycle mapping (downloading -> completed with content_path), the
//! ratio-gated `torrent-remove`, optional HTTP Basic auth, and the
//! not-found/failed/foreign-label edges.

mod common;

use cellarr_core::DownloadState;
use cellarr_download::{
    DownloadError, HttpTransport, RemovePolicy, TransmissionClient, TransmissionSettings,
};
use common::{torrent_grab, ReplayTransport};

fn settings() -> TransmissionSettings {
    TransmissionSettings {
        base_url: Some("http://localhost:9091".into()),
        host: None,
        port: None,
        url_base: None,
        download_dir: Some("/downloads".into()),
        username: None,
        password: None,
    }
}

fn settings_with_auth() -> TransmissionSettings {
    TransmissionSettings {
        base_url: Some("http://localhost:9091".into()),
        host: None,
        port: None,
        url_base: None,
        download_dir: None,
        username: Some("transmission".into()),
        password: Some("secret".into()),
    }
}

/// A transport wrapper so the test can keep an `Arc` to call `assert_drained`
/// while the adapter owns a `Box<dyn HttpTransport>`.
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
    settings: TransmissionSettings,
) -> (TransmissionClient, std::sync::Arc<ReplayTransport>) {
    let transport = std::sync::Arc::new(ReplayTransport::load(fixture));
    let client = TransmissionClient::with_transport(
        "transmission",
        settings,
        category,
        Box::new(ArcTransport(transport.clone())),
    );
    (client, transport)
}

fn client(fixture: &str, category: &str) -> (TransmissionClient, std::sync::Arc<ReplayTransport>) {
    client_with(fixture, category, settings())
}

#[tokio::test]
async fn full_lifecycle_handshake_paused_add_and_ratio_gated_remove() {
    let (client, transport) = client("transmission/lifecycle.json", "cellarr-tv");
    let grab = torrent_grab(
        "magnet:?xt=urn:btih:deadbeefcafef00d&dn=Show.S01E01",
        "cellarr-tv",
    );

    // The first call is answered 409; the adapter captures the session id and
    // retries the same torrent-add, which returns the hashString.
    let id = client.add(&grab, true).await.expect("add (paused)");
    assert_eq!(id, "deadbeefcafef00d");

    // Still downloading: no content_path yet, category from labels.
    let p = client.progress(&id).await.expect("progress downloading");
    assert_eq!(p.state, DownloadState::Downloading);
    assert!(p.content_path.is_none());
    assert!(p.is_in_category("cellarr-tv"));

    // Seeding/completed, but ratio 1.5 < 2.0 and time 7200 < 86400: not removable.
    let policy = RemovePolicy {
        min_ratio: Some(2.0),
        min_seeding_time_secs: Some(86_400),
        delete_data: true,
    };
    let removed = client.remove(&id, policy).await.expect("remove attempt 1");
    assert!(!removed, "should not remove before ratio/time gate met");

    // Ratio now 2.5 >= 2.0: removable. content_path = downloadDir/name.
    let removed = client.remove(&id, policy).await.expect("remove attempt 2");
    assert!(removed, "should remove once ratio gate satisfied");

    transport.assert_drained();
}

#[tokio::test]
async fn completed_status_projects_content_path_to_core() {
    let (client, transport) = client("transmission/lifecycle.json", "cellarr-tv");
    let grab = torrent_grab(
        "magnet:?xt=urn:btih:deadbeefcafef00d&dn=Show.S01E01",
        "cellarr-tv",
    );
    let _ = client.add(&grab, true).await.expect("add");
    // Drain the downloading row.
    let _ = client
        .progress("deadbeefcafef00d")
        .await
        .expect("downloading");

    // The completed row carries content_path = downloadDir + "/" + name and the
    // seed signals the executor needs for gated removal.
    let p = client
        .progress("deadbeefcafef00d")
        .await
        .expect("completed");
    assert_eq!(p.state, DownloadState::Completed);
    let core = p.to_core_status();
    assert!(core.is_completed());
    assert_eq!(
        core.content_path.as_deref(),
        Some("/downloads/cellarr-tv/Show.S01E01")
    );
    assert_eq!(core.ratio, Some(1.5));
    assert_eq!(core.seeding_time_secs, Some(7200));

    // The final ratio-gated remove with an immediate policy drains the fixture.
    client
        .remove("deadbeefcafef00d", RemovePolicy::immediate(true))
        .await
        .expect("remove");
    transport.assert_drained();
}

#[tokio::test]
async fn session_id_rotation_is_recaptured_and_basic_auth_sent() {
    let (client, transport) = client_with(
        "transmission/session_refresh_with_auth.json",
        "cellarr-movies",
        settings_with_auth(),
    );

    // First status: 409 -> capture "first-id" -> retry -> downloading.
    let s = client.status("abc123").await.expect("status 1");
    assert_eq!(s, DownloadState::Downloading);

    // Second status: the daemon rotated the id -> 409 with "rotated-id" -> the
    // adapter recaptures and retries -> completed.
    let s = client.status("abc123").await.expect("status 2");
    assert_eq!(s, DownloadState::Completed);

    transport.assert_drained();
}

#[tokio::test]
async fn errored_torrent_is_failed_and_foreign_label_is_visible() {
    let (client, transport) = client("transmission/status_errored_and_foreign.json", "cellarr-tv");

    // A non-empty errorString is a hard failure regardless of status.
    let p = client.progress("erroredhash").await.expect("errored");
    assert_eq!(p.state, DownloadState::Failed);

    // A completed torrent under a foreign label surfaces that label so the caller
    // can refuse to touch it (category scoping), with content_path from
    // downloadDir/name.
    let p = client.progress("foreignhash").await.expect("foreign");
    assert_eq!(p.state, DownloadState::Completed);
    assert!(!p.is_in_category("cellarr-tv"));
    assert!(p.is_in_category("manual-stuff"));
    assert_eq!(p.content_path.as_deref(), Some("/other/thing"));
    assert_eq!(p.ratio, Some(3.0));
    assert_eq!(p.seeding_time_secs, Some(100_000));

    transport.assert_drained();
}

#[tokio::test]
async fn empty_torrents_array_maps_to_not_found() {
    let (client, transport) = client("transmission/status_not_found.json", "cellarr-tv");
    let err = client.status("missing").await.unwrap_err();
    assert!(matches!(err, DownloadError::NotFound(_)), "got {err:?}");
    transport.assert_drained();
}
