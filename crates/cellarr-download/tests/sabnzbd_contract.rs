//! SABnzbd contract tests (record/replay; no live client).

mod common;

use cellarr_core::DownloadState;
use cellarr_download::{DownloadError, HttpTransport, SabnzbdClient, SabnzbdSettings};
use common::{usenet_grab, ReplayTransport};

fn settings() -> SabnzbdSettings {
    SabnzbdSettings {
        base_url: "http://localhost:8080".into(),
        api_key: "testkey".into(),
    }
}

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

fn client(fixture: &str, category: &str) -> (SabnzbdClient, std::sync::Arc<ReplayTransport>) {
    let transport = std::sync::Arc::new(ReplayTransport::load(fixture));
    let client = SabnzbdClient::with_transport(
        "sab",
        settings(),
        category,
        Box::new(ArcTransport(transport.clone())),
    );
    (client, transport)
}

#[tokio::test]
async fn full_lifecycle_completes_only_after_unpack() {
    let (client, transport) = client("sabnzbd/lifecycle.json", "cellarr-movies");
    let grab = usenet_grab("http://indexer/getnzb?id=1", "cellarr-movies");

    let id = client.add(&grab).await.expect("add");
    assert_eq!(id, "SABnzbd_nzo_abc123");

    // In queue, downloading.
    let p = client.progress(&id).await.expect("queue downloading");
    assert_eq!(p.state, DownloadState::Downloading);
    assert!((p.progress - 0.55).abs() < 1e-9);
    assert!(p.is_in_category("cellarr-movies"));

    // Still in queue, now extracting (post-process): NOT yet importable.
    let p = client.progress(&id).await.expect("queue extracting");
    assert_eq!(p.state, DownloadState::Downloading);
    assert!(p.content_path.is_none());

    // Left queue, now Completed in history with the unpacked path.
    let p = client.progress(&id).await.expect("history completed");
    assert_eq!(p.state, DownloadState::Completed);
    assert_eq!(
        p.content_path.as_deref(),
        Some("/downloads/complete/Movie.2024.1080p")
    );

    // The core projection a completed download exposes to the executor carries
    // that same on-disk path (required for Import). Usenet does not seed, so it
    // carries no ratio/seeding signal.
    let core = p.to_core_status();
    assert!(core.is_completed());
    assert_eq!(
        core.content_path.as_deref(),
        Some("/downloads/complete/Movie.2024.1080p")
    );
    assert_eq!(core.ratio, None);
    assert_eq!(core.seeding_time_secs, None);

    client.remove(&id, true).await.expect("remove");
    transport.assert_drained();
}

#[tokio::test]
async fn bad_api_key_maps_to_auth_error() {
    let (client, _t) = client("sabnzbd/auth_failure.json", "cellarr-movies");
    let grab = usenet_grab("http://indexer/getnzb?id=1", "cellarr-movies");
    let err = client.add(&grab).await.unwrap_err();
    assert!(matches!(err, DownloadError::Auth(_)), "got {err:?}");
}

#[tokio::test]
async fn failed_history_status_maps_to_failed() {
    let (client, transport) = client("sabnzbd/failed_download.json", "cellarr-tv");
    let p = client.progress("SABnzbd_nzo_bad").await.expect("progress");
    assert_eq!(p.state, DownloadState::Failed);
    transport.assert_drained();
}
