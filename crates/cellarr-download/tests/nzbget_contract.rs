//! NZBGet contract tests (record/replay; no live client).

mod common;

use cellarr_core::DownloadStatus;
use cellarr_download::{DownloadError, HttpTransport, NzbgetClient, NzbgetSettings};
use common::{usenet_grab, ReplayTransport};

fn settings() -> NzbgetSettings {
    NzbgetSettings {
        base_url: "http://localhost:6789".into(),
        username: "nzbget".into(),
        password: "tegbzn6789".into(),
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

fn client(fixture: &str, category: &str) -> (NzbgetClient, std::sync::Arc<ReplayTransport>) {
    let transport = std::sync::Arc::new(ReplayTransport::load(fixture));
    let client = NzbgetClient::with_transport(
        "nzbget",
        settings(),
        category,
        Box::new(ArcTransport(transport.clone())),
    );
    (client, transport)
}

#[tokio::test]
async fn full_lifecycle_completes_only_after_postprocess() {
    let (client, transport) = client("nzbget/lifecycle.json", "cellarr-tv");
    let grab = usenet_grab("http://indexer/getnzb?id=7", "cellarr-tv");

    let id = client.add(&grab).await.expect("append");
    assert_eq!(id, "42");

    // listgroups: downloading.
    let p = client.progress(&id).await.expect("downloading");
    assert_eq!(p.status, DownloadStatus::Downloading);
    assert!((p.progress - 0.6).abs() < 1e-9);
    assert!(p.is_in_category("cellarr-tv"));

    // listgroups: unpacking (post-process), still not importable.
    let p = client.progress(&id).await.expect("unpacking");
    assert_eq!(p.status, DownloadStatus::Downloading);
    assert!(p.content_path.is_none());

    // history: SUCCESS with DestDir.
    let p = client.progress(&id).await.expect("history success");
    assert_eq!(p.status, DownloadStatus::Completed);
    assert_eq!(
        p.content_path.as_deref(),
        Some("/downloads/dst/Show.S02E05")
    );

    client.remove(&id, true).await.expect("remove");
    transport.assert_drained();
}

#[tokio::test]
async fn http_401_maps_to_auth_error() {
    let (client, _t) = client("nzbget/auth_401.json", "cellarr-tv");
    let grab = usenet_grab("http://indexer/getnzb?id=7", "cellarr-tv");
    let err = client.add(&grab).await.unwrap_err();
    assert!(matches!(err, DownloadError::Auth(_)), "got {err:?}");
}

#[tokio::test]
async fn history_failure_status_maps_to_failed() {
    let (client, transport) = client("nzbget/failed_download.json", "cellarr-tv");
    let p = client.progress("99").await.expect("progress");
    assert_eq!(p.status, DownloadStatus::Failed);
    transport.assert_drained();
}
