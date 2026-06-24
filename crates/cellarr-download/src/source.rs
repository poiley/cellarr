//! Topology-independent torrent-source resolution.
//!
//! # The bug this fixes
//!
//! A release's `download_url` is whatever the indexer advertised. For a Prowlarr
//! Torznab feed that is **not** a magnet — it is an HTTP *proxy* URL on the
//! Prowlarr host (`http://<prowlarr>/<id>/download?apikey=…&link=…`) that 3xx
//! redirects to the real magnet, or streams the `.torrent` bytes. cellarr reaches
//! Prowlarr through a `kubectl port-forward` at `http://127.0.0.1:<port>`, so the
//! URL it holds is only resolvable **on the cellarr host**.
//!
//! The torrent adapters used to hand that URL straight to the download client
//! (Transmission `torrent-add` `filename=<url>`, qBittorrent `urls=<url>`) and let
//! the client fetch it. But the client runs elsewhere — in-cluster, behind a VPN —
//! where `127.0.0.1:<port>` is *its own* localhost, not Prowlarr. The fetch fails
//! and the torrent never adds. The same latent break hits any client that cannot
//! reach the indexer the way cellarr can.
//!
//! # The fix
//!
//! cellarr — which *can* reach the indexer — resolves the `download_url` to a
//! concrete, self-contained [`TorrentSource`] **before** talking to the client,
//! and hands the client only that. The client then never needs to reach the
//! indexer:
//!
//! - `magnet:` URL → [`TorrentSource::Magnet`] passthrough (already self-contained).
//! - `http(s)` URL → cellarr GETs it (redirects **not** auto-followed):
//!   - a 3xx whose `Location` is a `magnet:` → [`TorrentSource::Magnet`];
//!   - a 2xx body → the `.torrent` metainfo bytes → [`TorrentSource::Metainfo`];
//!   - a 3xx to another `http(s)` → follow it (bounded to [`MAX_REDIRECTS`] hops),
//!     then apply the same rules.

use crate::error::DownloadError;
use crate::http::{HttpRequest, HttpTransport};

/// The maximum number of `http(s)` → `http(s)` redirect hops the resolver will
/// follow before giving up. Bounds the work (and defeats redirect loops) so a
/// single grab can never wedge the pipeline.
pub const MAX_REDIRECTS: usize = 5;

/// A self-contained torrent source ready to hand to a download client.
///
/// Either form is submittable without the client needing to reach the indexer:
/// a magnet resolves via the client's own DHT/trackers, and metainfo is the
/// `.torrent` file's bytes uploaded directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TorrentSource {
    /// A `magnet:` URI to submit as-is.
    Magnet(String),
    /// The raw bytes of a `.torrent` (bencoded metainfo) to upload directly.
    Metainfo(Vec<u8>),
}

impl TorrentSource {
    /// Resolve a release's `download_url` into a submittable [`TorrentSource`],
    /// fetching through `transport` when the URL is `http(s)`.
    ///
    /// See the [module docs](self) for the resolution rules. `transport`'s
    /// [`send_raw`](HttpTransport::send_raw) must not auto-follow redirects — the
    /// production [`ReqwestTransport`](crate::http::ReqwestTransport) disables them.
    ///
    /// # Errors
    /// [`DownloadError::Config`] for an empty/unsupported URL scheme;
    /// [`DownloadError::Api`] for a redirect with no usable `Location`, an
    /// `http(s)` response that is neither a redirect nor a success, or too many
    /// redirect hops; [`DownloadError::Transport`] for a transport failure.
    pub async fn resolve(
        download_url: &str,
        transport: &dyn HttpTransport,
    ) -> Result<TorrentSource, DownloadError> {
        let url = download_url.trim();
        if url.is_empty() {
            return Err(DownloadError::Config(
                "release has an empty download_url; nothing to resolve".into(),
            ));
        }
        if is_magnet(url) {
            return Ok(TorrentSource::Magnet(url.to_string()));
        }
        if !is_http(url) {
            return Err(DownloadError::Config(format!(
                "unsupported download_url scheme (not magnet: or http(s)): {url}"
            )));
        }

        // Follow http(s) → http(s) hops ourselves, bounded, so the client never
        // has to. A magnet Location ends the walk with a magnet; a 2xx body ends
        // it with metainfo.
        let mut current = url.to_string();
        for _ in 0..=MAX_REDIRECTS {
            let resp = transport
                .send_raw(HttpRequest::new("GET", current.clone()))
                .await?;

            if resp.is_redirect() {
                let location = resp
                    .header("location")
                    .map(str::trim)
                    .filter(|l| !l.is_empty());
                let Some(location) = location else {
                    return Err(DownloadError::Api(format!(
                        "download_url redirected ({}) with no usable Location header",
                        resp.status
                    )));
                };
                let next = resolve_location(&current, location);
                if is_magnet(&next) {
                    return Ok(TorrentSource::Magnet(next));
                }
                if !is_http(&next) {
                    return Err(DownloadError::Api(format!(
                        "download_url redirected to an unsupported scheme: {next}"
                    )));
                }
                current = next;
                continue;
            }

            if resp.is_success() {
                if resp.body.is_empty() {
                    return Err(DownloadError::Api(
                        "download_url returned an empty body (no .torrent metainfo)".into(),
                    ));
                }
                return Ok(TorrentSource::Metainfo(resp.body));
            }

            return Err(DownloadError::Api(format!(
                "download_url fetch failed (status {})",
                resp.status
            )));
        }

        Err(DownloadError::Api(format!(
            "download_url exceeded {MAX_REDIRECTS} redirect hops without resolving"
        )))
    }
}

/// Whether `url` is a magnet URI (case-insensitive scheme).
fn is_magnet(url: &str) -> bool {
    url.len() >= "magnet:".len() && url[.."magnet:".len()].eq_ignore_ascii_case("magnet:")
}

/// Whether `url` is an `http`/`https` URL (case-insensitive scheme).
fn is_http(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Resolve a (possibly relative) redirect `location` against the `base` request
/// URL. A magnet or an absolute `http(s)` location is used as-is; a root-relative
/// or relative path is joined onto the base's origin/path so a `Location: /dl/x`
/// from an HTTP indexer still points back at that indexer.
fn resolve_location(base: &str, location: &str) -> String {
    if is_magnet(location) || is_http(location) {
        return location.to_string();
    }
    // Relative redirect: resolve against the base. Split the base into its origin
    // (scheme://host[:port]) and path so both root-relative ("/x") and
    // path-relative ("x") forms can be joined.
    let Some(scheme_end) = base.find("://") else {
        return location.to_string();
    };
    let after_scheme = scheme_end + "://".len();
    let origin_len = base[after_scheme..]
        .find('/')
        .map_or(base.len(), |i| after_scheme + i);
    let origin = &base[..origin_len];

    if let Some(stripped) = location.strip_prefix('/') {
        return format!("{origin}/{stripped}");
    }
    // Path-relative: replace the last path segment of the base.
    let base_path = &base[origin_len..];
    let dir_end = base_path.rfind('/').map_or(0, |i| i + 1);
    format!("{origin}{}{location}", &base_path[..dir_end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnet_detection_is_case_insensitive() {
        assert!(is_magnet("magnet:?xt=urn:btih:abc"));
        assert!(is_magnet("MAGNET:?xt=urn:btih:abc"));
        assert!(!is_magnet("http://x/y"));
        assert!(!is_magnet("mag"));
    }

    #[test]
    fn http_detection() {
        assert!(is_http("http://x"));
        assert!(is_http("HTTPS://x"));
        assert!(!is_http("magnet:?xt=urn:btih:abc"));
        assert!(!is_http("ftp://x"));
    }

    #[test]
    fn absolute_and_magnet_locations_pass_through() {
        assert_eq!(
            resolve_location("http://idx/1/download", "magnet:?xt=urn:btih:abc"),
            "magnet:?xt=urn:btih:abc"
        );
        assert_eq!(
            resolve_location("http://idx/1/download", "https://cdn/x.torrent"),
            "https://cdn/x.torrent"
        );
    }

    #[test]
    fn root_relative_location_joins_origin() {
        assert_eq!(
            resolve_location("http://idx:9696/1/download?a=b", "/dl/real.torrent"),
            "http://idx:9696/dl/real.torrent"
        );
    }

    #[test]
    fn path_relative_location_replaces_last_segment() {
        assert_eq!(
            resolve_location("http://idx/a/b/download", "real.torrent"),
            "http://idx/a/b/real.torrent"
        );
    }
}
