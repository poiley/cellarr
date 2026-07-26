//! Download-link resolution for trackers whose listing pages omit the link.
//!
//! Most Cardigann definitions carry the download link (or a bare infohash) in the
//! search row itself, so [`crate::CardigannIndexer`] can build a magnet without a
//! second request. A tracker that hands its magnets out through a signed endpoint
//! instead leaves that field empty, and the row would be dropped for want of a
//! link — the release is otherwise fully parsed.
//!
//! A [`DownloadResolver`] fills that gap: given the release's details URL, it
//! performs whatever extra requests the tracker needs and returns a usable link.
//! Resolution is per-release and only runs for rows that arrived without one, so a
//! definition that already supplies links never pays for it.

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::error::{IndexerError, Result};
use crate::http::Fetcher;

/// Resolves a release's download link when the search row didn't carry one.
#[async_trait]
pub trait DownloadResolver: Send + Sync {
    /// Whether this resolver handles releases from `details_url`.
    ///
    /// Called once per link-less row; a `false` here drops the row exactly as it
    /// would have been dropped without any resolver configured.
    fn handles(&self, details_url: &str) -> bool;

    /// Resolve `details_url` into a download link (a magnet URI or an `http(s)`
    /// URL), issuing whatever requests the tracker requires through `fetcher`.
    async fn resolve(&self, details_url: &str, fetcher: &dyn Fetcher) -> Result<String>;
}

/// Resolver for EXT Torrents, which serves magnets from a signed AJAX endpoint.
///
/// The listing markup carries only a numeric torrent id (`data-id`), and the
/// magnet is fetched by `POST`ing that id to `/ajax/getTorrentMagnet.php` together
/// with a signature over `torrent_id|timestamp|pageToken`. Both `pageToken` and
/// `csrfToken` are rendered inline into every page, so one `GET` of the details
/// page supplies everything the signature needs.
///
/// The site's own client calls this a "HMAC"; it is a plain SHA-256 digest of the
/// joined triple, with no keying beyond the per-page token.
pub struct ExtTorrentsResolver {
    /// Host suffixes this resolver claims. EXT Torrents publishes several mirror
    /// domains that serve the same backend, so matching is by suffix rather than
    /// against one canonical host.
    host_suffixes: Vec<String>,
}

impl Default for ExtTorrentsResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtTorrentsResolver {
    /// The signed endpoint that trades a torrent id for a magnet.
    const MAGNET_PATH: &'static str = "/ajax/getTorrentMagnet.php";

    /// Build a resolver covering the tracker's published mirror domains.
    #[must_use]
    pub fn new() -> Self {
        Self {
            host_suffixes: vec![
                "ext.to".to_string(),
                "extto.com".to_string(),
                "torrentbay.st".to_string(),
            ],
        }
    }

    /// Build a resolver matching an explicit set of host suffixes.
    ///
    /// Lets an operator point the resolver at a mirror that isn't in the built-in
    /// list without waiting for a release.
    #[must_use]
    pub fn with_hosts(host_suffixes: Vec<String>) -> Self {
        Self { host_suffixes }
    }

    /// The trailing numeric id in a details URL (`/some-release-title-16174236/`).
    ///
    /// The id is the same value the listing markup exposes as `data-id`, so it can
    /// be recovered from the details URL alone and no listing-side field is needed.
    fn torrent_id(details_url: &str) -> Option<&str> {
        let path = details_url.split(['?', '#']).next()?;
        let slug = path.trim_end_matches('/').rsplit('/').next()?;
        let id = slug.rsplit('-').next()?;
        (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then_some(id)
    }

    /// Read `window.<name> = '<hex>'` out of a page's inline script.
    fn page_token(html: &str, name: &str) -> Option<String> {
        let needle = format!("window.{name}");
        let rest = &html[html.find(&needle)? + needle.len()..];
        let rest = rest.trim_start().strip_prefix('=')?.trim_start();
        let quote = rest.chars().next().filter(|c| *c == '\'' || *c == '"')?;
        let value: String = rest[1..].chars().take_while(|c| *c != quote).collect();
        (!value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(value)
    }

    /// Sign a request the way the tracker's client does: SHA-256 over the joined
    /// `id|timestamp|token`, lowercase hex.
    fn sign(torrent_id: &str, timestamp: u64, page_token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(format!("{torrent_id}|{timestamp}|{page_token}").as_bytes());
        hasher
            .finalize()
            .iter()
            .fold(String::with_capacity(64), |mut acc, b| {
                use std::fmt::Write as _;
                let _ = write!(acc, "{b:02x}");
                acc
            })
    }

    /// Pull the magnet out of the endpoint's `{"success":true,"magnet":"…"}` reply.
    fn magnet_from_reply(body: &str) -> Result<String> {
        let reply: serde_json::Value = serde_json::from_str(body)
            .map_err(|e| IndexerError::Parse(format!("magnet endpoint reply: {e}")))?;
        if reply.get("success").and_then(serde_json::Value::as_bool) != Some(true) {
            let reason = reply
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("no reason given");
            return Err(IndexerError::Parse(format!(
                "magnet endpoint refused: {reason}"
            )));
        }
        reply
            .get("magnet")
            .and_then(serde_json::Value::as_str)
            .filter(|m| m.starts_with("magnet:"))
            .map(str::to_string)
            .ok_or_else(|| IndexerError::Parse("magnet endpoint reply had no magnet".to_string()))
    }

    /// Seconds since the Unix epoch, as the signature's timestamp component.
    ///
    /// A clock before the epoch yields `0`, which the tracker rejects as a stale
    /// signature — a failed resolve, never a panic.
    fn now_unix() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }

    /// `scheme://host` of `url`, used to address the endpoint on whichever mirror
    /// the release came from.
    fn origin(url: &str) -> Option<String> {
        let (scheme, rest) = url.split_once("://")?;
        let host = rest.split('/').next()?;
        (!host.is_empty()).then(|| format!("{scheme}://{host}"))
    }
}

#[async_trait]
impl DownloadResolver for ExtTorrentsResolver {
    fn handles(&self, details_url: &str) -> bool {
        let Some(rest) = details_url.split("://").nth(1) else {
            return false;
        };
        let host = rest.split('/').next().unwrap_or_default();
        let host = host.split(':').next().unwrap_or_default();
        self.host_suffixes
            .iter()
            .any(|suffix| host == suffix || host.ends_with(&format!(".{suffix}")))
    }

    async fn resolve(&self, details_url: &str, fetcher: &dyn Fetcher) -> Result<String> {
        let torrent_id = Self::torrent_id(details_url).ok_or_else(|| {
            IndexerError::Parse(format!("no torrent id in details URL {details_url}"))
        })?;
        let origin = Self::origin(details_url)
            .ok_or_else(|| IndexerError::Parse(format!("unusable details URL {details_url}")))?;

        let page = fetcher.get(details_url).await?;
        let page_token = Self::page_token(&page, "pageToken")
            .ok_or_else(|| IndexerError::Parse("details page carried no pageToken".to_string()))?;
        // The endpoint checks the session id separately from the signature, so a
        // page without one cannot be signed into a valid request.
        let csrf_token = Self::page_token(&page, "csrfToken")
            .ok_or_else(|| IndexerError::Parse("details page carried no csrfToken".to_string()))?;

        let timestamp = Self::now_unix();
        let signature = Self::sign(torrent_id, timestamp, &page_token);
        let form = format!(
            "torrent_id={torrent_id}&action=get_magnet&timestamp={timestamp}&hmac={signature}&sessid={csrf_token}"
        );

        let reply = fetcher
            .post(
                &format!("{origin}{}", Self::MAGNET_PATH),
                &form,
                "application/x-www-form-urlencoded",
            )
            .await?;
        Self::magnet_from_reply(&reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torrent_id_is_the_trailing_number_of_the_slug() {
        assert_eq!(
            ExtTorrentsResolver::torrent_id("https://ext.to/1883-s01e01-2160p-playweb-16174236/"),
            Some("16174236")
        );
        assert_eq!(
            ExtTorrentsResolver::torrent_id("https://ext.to/some-release-42/?utm=x"),
            Some("42")
        );
    }

    #[test]
    fn torrent_id_rejects_a_slug_without_a_numeric_tail() {
        assert_eq!(
            ExtTorrentsResolver::torrent_id("https://ext.to/browse/"),
            None
        );
        assert_eq!(ExtTorrentsResolver::torrent_id("https://ext.to/"), None);
    }

    #[test]
    fn page_token_reads_either_quote_style() {
        let html = "<script>window.pageToken = 'bd10d1c719b340791e1d11cd271a7ca0';</script>";
        assert_eq!(
            ExtTorrentsResolver::page_token(html, "pageToken").as_deref(),
            Some("bd10d1c719b340791e1d11cd271a7ca0")
        );
        let html = r#"window.csrfToken="a0e4b5e37e90ee06e6232b6a72498804""#;
        assert_eq!(
            ExtTorrentsResolver::page_token(html, "csrfToken").as_deref(),
            Some("a0e4b5e37e90ee06e6232b6a72498804")
        );
    }

    #[test]
    fn page_token_is_absent_when_the_page_does_not_declare_it() {
        assert_eq!(
            ExtTorrentsResolver::page_token("<html><body>nothing</body></html>", "pageToken"),
            None
        );
    }

    /// Pins the signature against a triple whose digest was taken from the
    /// tracker's own client, so a refactor of the join order is caught.
    #[test]
    fn signature_is_sha256_over_id_timestamp_token() {
        assert_eq!(
            ExtTorrentsResolver::sign("16174236", 1785107493, "bd10d1c719b340791e1d11cd271a7ca0"),
            "8fef585b09b263a7af908658b466e0e74fb2b089e66aed2cb885a1e982c2a5b7"
        );
    }

    #[test]
    fn magnet_is_read_from_a_successful_reply() {
        let body = r#"{"success":true,"magnet":"magnet:?xt=urn:btih:E4F4A432&dn=x"}"#;
        assert_eq!(
            ExtTorrentsResolver::magnet_from_reply(body).unwrap(),
            "magnet:?xt=urn:btih:E4F4A432&dn=x"
        );
    }

    #[test]
    fn a_refused_reply_surfaces_the_trackers_reason() {
        let body = r#"{"success":false,"error":"Invalid session"}"#;
        let err = ExtTorrentsResolver::magnet_from_reply(body).unwrap_err();
        assert!(err.to_string().contains("Invalid session"), "{err}");
    }

    #[test]
    fn handles_matches_published_mirrors_but_not_look_alikes() {
        let r = ExtTorrentsResolver::new();
        assert!(r.handles("https://ext.to/a-1/"));
        assert!(r.handles("https://search.extto.com/a-1/"));
        assert!(r.handles("https://extranet.torrentbay.st/a-1/"));
        assert!(!r.handles("https://notext.to.evil.com/a-1/"));
        assert!(!r.handles("https://thepiratebay.org/a-1/"));
    }
}
