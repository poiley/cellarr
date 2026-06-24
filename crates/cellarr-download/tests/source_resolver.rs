//! Topology-independent torrent-source resolution (record/replay; no live host).
//!
//! Pins the four resolution paths the Prowlarr/port-forward bug requires cellarr
//! to handle itself so the download client never has to reach the indexer:
//!   - a `magnet:` `download_url` passes through with **no** HTTP call;
//!   - an `http` URL that streams `.torrent` bytes -> `Metainfo`;
//!   - an `http` URL that 3xx-redirects to a `magnet:` `Location` -> `Magnet`;
//!   - an `http` URL that redirects through bounded `http`->`http` hops -> the
//!     final `.torrent` `Metainfo`.

mod common;

use cellarr_download::source::TorrentSource;
use cellarr_download::HttpTransport;
use cellarr_download::{DownloadError, HttpRequest, HttpResponse};
use common::ReplayTransport;

/// A transport wrapper so the test can keep an `Arc` for `assert_drained`.
struct ArcTransport(std::sync::Arc<ReplayTransport>);

#[async_trait::async_trait]
impl HttpTransport for ArcTransport {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, DownloadError> {
        self.0.send(req).await
    }
}

fn transport(fixture: &str) -> (ArcTransport, std::sync::Arc<ReplayTransport>) {
    let t = std::sync::Arc::new(ReplayTransport::load(fixture));
    (ArcTransport(t.clone()), t)
}

#[tokio::test]
async fn magnet_url_passes_through_without_any_http_call() {
    // No fixture exchanges: a magnet must resolve with zero transport traffic.
    let (t, transport) = transport("source/no_exchanges.json");
    let magnet = "magnet:?xt=urn:btih:deadbeefcafef00d&dn=Show.S01E01";
    let source = TorrentSource::resolve(magnet, &t).await.expect("resolve");
    assert_eq!(source, TorrentSource::Magnet(magnet.to_string()));
    transport.assert_drained();
}

#[tokio::test]
async fn http_url_streaming_torrent_bytes_resolves_to_metainfo() {
    let (t, transport) = transport("source/http_to_torrent.json");
    let url = "http://127.0.0.1:19696/1/download?apikey=KEY&link=LINK";
    let source = TorrentSource::resolve(url, &t).await.expect("resolve");
    match source {
        TorrentSource::Metainfo(bytes) => {
            assert!(bytes.starts_with(b"d8:announce"), "got {bytes:?}");
            assert!(bytes.ends_with(b"ee"));
        }
        other => panic!("expected metainfo, got {other:?}"),
    }
    transport.assert_drained();
}

#[tokio::test]
async fn http_url_redirecting_to_magnet_resolves_to_magnet() {
    let (t, transport) = transport("source/http_to_magnet_redirect.json");
    let url = "http://127.0.0.1:19696/1/download?apikey=KEY&link=LINK";
    let source = TorrentSource::resolve(url, &t).await.expect("resolve");
    assert_eq!(
        source,
        TorrentSource::Magnet("magnet:?xt=urn:btih:deadbeefcafef00d&dn=Show.S01E01".into())
    );
    transport.assert_drained();
}

#[tokio::test]
async fn bounded_http_to_http_redirects_then_metainfo() {
    let (t, transport) = transport("source/http_chain_to_torrent.json");
    let url = "http://127.0.0.1:19696/1/download?apikey=KEY&link=LINK";
    let source = TorrentSource::resolve(url, &t).await.expect("resolve");
    match source {
        TorrentSource::Metainfo(bytes) => assert!(bytes.starts_with(b"d8:announce")),
        other => panic!("expected metainfo, got {other:?}"),
    }
    transport.assert_drained();
}

#[tokio::test]
async fn empty_download_url_is_a_config_error() {
    let (t, _transport) = transport("source/no_exchanges.json");
    let err = TorrentSource::resolve("   ", &t).await.unwrap_err();
    assert!(matches!(err, DownloadError::Config(_)), "got {err:?}");
}
