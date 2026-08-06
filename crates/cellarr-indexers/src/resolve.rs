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

use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::cardigann::Definition;
use crate::error::{IndexerError, Result};
use crate::http::Fetcher;

/// The resolver a definition needs, if any.
///
/// Keeps per-tracker knowledge in this crate: the integration layer asks for a
/// definition's resolver rather than knowing which trackers are special. A
/// definition with no entry here resolves nothing, which is the ordinary case —
/// most trackers publish the link in the row.
///
/// The resolver is scoped to the hosts the *definition* declares, so an operator
/// who adds a mirror to their copy gets it covered without a code change.
#[must_use]
pub fn resolver_for(definition: &Definition) -> Option<Arc<dyn DownloadResolver>> {
    match definition.id.as_str() {
        "exttorrents" => {
            let hosts = definition
                .links
                .iter()
                .filter_map(|link| Some(link.split("://").nth(1)?.split('/').next()?.to_string()))
                .filter(|host| !host.is_empty())
                .collect::<Vec<_>>();
            let resolver = if hosts.is_empty() {
                ExtTorrentsResolver::new()
            } else {
                ExtTorrentsResolver::with_hosts(hosts)
            };
            Some(Arc::new(resolver))
        }
        _ => None,
    }
}

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

    /// Pull the magnet out of a successful reply.
    ///
    /// The endpoint carries the link in `url`, and separately may carry the raw
    /// infohash in `hash`. Its own client prefers `url` and falls back to building
    /// `magnet:?xt=urn:btih:<hash>` when `url` is absent — a partial reply that
    /// still identifies the torrent, and which it takes even when an `error` string
    /// rides along beside it. Both shapes are accepted here for the same reason: a
    /// resolvable infohash is a working magnet, and refusing it would drop releases
    /// the tracker was willing to serve.
    ///
    /// `magnet` is accepted too, ahead of both, so a mirror still answering with the
    /// older key keeps working.
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
        let direct = ["magnet", "url"].into_iter().find_map(|key| {
            reply
                .get(key)
                .and_then(serde_json::Value::as_str)
                .filter(|m| m.starts_with("magnet:"))
                .map(str::to_string)
        });
        if let Some(magnet) = direct {
            return Ok(magnet);
        }
        reply
            .get("hash")
            .and_then(serde_json::Value::as_str)
            .filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_alphanumeric()))
            .map(|h| format!("magnet:?xt=urn:btih:{h}"))
            .ok_or_else(|| {
                IndexerError::Parse("magnet endpoint reply had no link or hash".to_string())
            })
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
        // The magnet request is signed against tokens the details page issued, so
        // the two trips have to land on the same session. Serializing individual
        // requests does not achieve that: it stops them overlapping, but still lets
        // an unrelated search land between them and rotate the session, after which
        // the endpoint answers "Invalid session" — a complaint about our tokens,
        // not about the release.
        //
        // Holding the session across the pair is the fix. A retry with fresh tokens
        // was tried first and recovered nothing (0 of 26 in production): a retry
        // races for the session on exactly the same terms as the request that just
        // lost it, so under steady search traffic it loses again.
        fetcher
            .in_session(Box::pin(self.resolve_once(details_url, fetcher)))
            .await
    }
}

impl ExtTorrentsResolver {
    /// Whether the endpoint refused because our session tokens are stale, rather
    /// than because anything is wrong with the release.
    fn is_stale_session(err: &IndexerError) -> bool {
        let text = err.to_string().to_ascii_lowercase();
        text.contains("invalid session") || text.contains("refresh the page")
    }

    /// One full attempt: fetch the details page for fresh tokens, sign, and ask.
    async fn resolve_once(&self, details_url: &str, fetcher: &dyn Fetcher) -> Result<String> {
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
        // `download_type` is what the endpoint reads to decide which link to hand
        // back; it is not an optional hint. An earlier `action=get_magnet` was not a
        // parameter the endpoint has at all, so the field it does look for arrived
        // empty.
        let form = format!(
            "torrent_id={torrent_id}&download_type=magnet&timestamp={timestamp}&hmac={signature}&sessid={csrf_token}"
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

    /// The shape the endpoint actually answers with: the link arrives under `url`.
    /// Reading only `magnet` meant every resolve failed against a reply that was
    /// carrying a perfectly good link.
    #[test]
    fn the_link_is_read_from_the_url_field() {
        let body = r#"{"success":true,"type":"magnet","url":"magnet:?xt=urn:btih:E4F4A432&dn=x"}"#;
        assert_eq!(
            ExtTorrentsResolver::magnet_from_reply(body).unwrap(),
            "magnet:?xt=urn:btih:E4F4A432&dn=x"
        );
    }

    /// A reply with no link but a usable infohash still identifies the torrent, so
    /// it is built into a magnet rather than dropped — what the tracker's own client
    /// does, including when a non-fatal `error` rides along beside the hash.
    #[test]
    fn a_reply_with_only_an_infohash_still_yields_a_magnet() {
        let body = r#"{"success":true,"hash":"E4F4A432DEADBEEF","error":"cached copy"}"#;
        assert_eq!(
            ExtTorrentsResolver::magnet_from_reply(body).unwrap(),
            "magnet:?xt=urn:btih:E4F4A432DEADBEEF"
        );
    }

    /// A success carrying neither a link nor a hash is a failed resolve, not an
    /// empty magnet handed onward to the download client.
    #[test]
    fn a_reply_with_neither_link_nor_hash_is_an_error() {
        let err = ExtTorrentsResolver::magnet_from_reply(r#"{"success":true}"#).unwrap_err();
        assert!(err.to_string().contains("no link or hash"), "{err}");
    }

    /// A non-magnet `url` (an interstitial or ad redirect) must not be passed off as
    /// a magnet; the infohash beside it is the usable answer.
    #[test]
    fn a_non_magnet_url_falls_through_to_the_infohash() {
        let body = r#"{"success":true,"url":"https://ext.to/interstitial","hash":"ABC123"}"#;
        assert_eq!(
            ExtTorrentsResolver::magnet_from_reply(body).unwrap(),
            "magnet:?xt=urn:btih:ABC123"
        );
    }

    #[test]
    fn a_refused_reply_surfaces_the_trackers_reason() {
        let body = r#"{"success":false,"error":"Invalid session"}"#;
        let err = ExtTorrentsResolver::magnet_from_reply(body).unwrap_err();
        assert!(err.to_string().contains("Invalid session"), "{err}");
    }

    /// The registry scopes the resolver to the hosts the operator's own copy of
    /// the definition declares, so a mirror they add is covered without a release.
    #[test]
    fn resolver_for_scopes_to_the_definitions_own_links() {
        let def = Definition::from_yaml(
            r#"
id: exttorrents
name: EXT Torrents
links:
  - https://ext.to/
  - https://my-private-mirror.example/
search:
  paths:
    - path: /browse/
  rows:
    selector: tr
  fields:
    title:
      selector: a
"#,
        )
        .expect("parse definition");
        let resolver = resolver_for(&def).expect("exttorrents should have a resolver");
        assert!(resolver.handles("https://ext.to/a-1/"));
        assert!(resolver.handles("https://my-private-mirror.example/a-1/"));
        // A mirror this copy does not declare is not claimed.
        assert!(!resolver.handles("https://extranet.torrentbay.st/a-1/"));
    }

    #[test]
    fn resolver_for_is_none_for_an_ordinary_definition() {
        let def = Definition::from_yaml(
            r#"
id: ordinarytracker
name: Ordinary Tracker
links:
  - https://ordinary.example/
search:
  paths:
    - path: /browse/
  rows:
    selector: tr
  fields:
    title:
      selector: a
"#,
        )
        .expect("parse definition");
        assert!(resolver_for(&def).is_none());
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

#[cfg(test)]
mod stale_session_tests {
    use super::*;

    /// "Invalid session" means our tokens went stale between fetching the details
    /// page and asking for the magnet — nothing is wrong with the release, so it
    /// must be retried with fresh tokens rather than dropped.
    #[test]
    fn a_stale_session_refusal_is_recognised() {
        let err = IndexerError::Parse("magnet endpoint refused: Invalid session".to_string());
        assert!(ExtTorrentsResolver::is_stale_session(&err));
        let err = IndexerError::Parse(
            "magnet endpoint refused: Invalid request. Please refresh the page.".to_string(),
        );
        assert!(ExtTorrentsResolver::is_stale_session(&err));
    }

    /// A refusal that is ABOUT the release must not be retried — retrying would
    /// double the cost of every genuinely gone torrent against a tracker whose
    /// request budget is the scarce resource.
    #[test]
    fn a_refusal_about_the_release_is_not_retried() {
        for message in [
            "magnet endpoint refused: torrent not found",
            "magnet endpoint reply had no link or hash",
            "no torrent id in details URL https://ext.to/browse/",
        ] {
            let err = IndexerError::Parse(message.to_string());
            assert!(
                !ExtTorrentsResolver::is_stale_session(&err),
                "{message} is about the release, not the session"
            );
        }
    }
}
