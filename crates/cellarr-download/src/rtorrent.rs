//! rTorrent XML-RPC adapter.
//!
//! rTorrent has no JSON API: it speaks **XML-RPC**, reached either over a direct
//! HTTP XML-RPC mount (an `scgi`/`httprpc` front, e.g. ruTorrent's
//! `plugins/httprpc/action.php`, or an nginx `SCGI`-to-HTTP shim at `/RPC2`) (see
//! `docs/06-integrations.md`). The adapter speaks XML-RPC over the configured HTTP
//! endpoint through the same [`HttpTransport`] seam every other client uses, so it
//! is record/replay-testable with no live daemon.
//!
//! Method map (the rTorrent XML-RPC surface):
//! - **Add** via `load.start` (`load.start "" <uri/magnet>`) plus a `d.custom1.set`
//!   to stamp cellarr's label (ruTorrent stores the label in `d.custom1`). rTorrent
//!   identifies a torrent by its **infohash**, which cellarr derives from the
//!   resolved source (a magnet's `btih`, or the SHA-1 of the uploaded `.torrent`'s
//!   `info` dict) — the same derivation the qBittorrent adapter uses — because
//!   `load.start` does not return the hash. A `.torrent` is loaded as raw bytes via
//!   `load.raw_start` (the metainfo passed as an XML-RPC `<base64>` value).
//! - **Status** via `d.multicall2` selecting `d.hash`, `d.complete`, `d.ratio`,
//!   `d.base_path`, `d.name`, `d.custom1` (label), `d.message` (error), and
//!   `d.peers_complete`/`d.peers_accounted` (peers). rTorrent's `d.complete`
//!   (0/1) + `d.base_path` give the lifecycle state and the importable
//!   `content_path`; `d.ratio` is a permille integer (1000 = ratio 1.0).
//! - **Remove** via `d.erase <hash>`. rTorrent's `d.erase` only removes the
//!   torrent from the session; deleting the data is not a first-class flag, so when
//!   the policy asks to delete data the adapter best-effort issues a delete of the
//!   `d.base_path` is **not** attempted here (rTorrent has no safe RPC for it
//!   without a userscript) — see the deferred note below.
//!
//! ## Auth
//! rTorrent's XML-RPC has no native auth; deployments front it with HTTP Basic on
//! the web server (ruTorrent/nginx). The adapter sends `Authorization: Basic …`
//! whenever a username is configured and maps a `401` to [`DownloadError::Auth`].
//!
//! Category scoping: every add stamps cellarr's label into `d.custom1`, and status
//! surfaces that label so the caller can refuse to act on a foreign torrent.

use cellarr_core::{DownloadState, GrabRequest};

use crate::error::DownloadError;
use crate::http::{HttpRequest, HttpTransport};
use crate::lifecycle::{DownloadProgress, RemovePolicy};
use crate::source::TorrentSource;

/// Connection + auth settings for an rTorrent client, deserialized from a
/// [`cellarr_core::DownloadClientConfig`]'s `settings` JSON.
///
/// Two shapes are accepted (mirroring the other torrent adapters): a ready
/// `base_url`, or discrete `host`/`port`. `urlBase` is the XML-RPC mount path on
/// the front-end (e.g. `/RPC2`, or `/rutorrent/plugins/httprpc/action.php`),
/// defaulting to `/RPC2`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RtorrentSettings {
    /// Pre-assembled base URL of the XML-RPC host, e.g. `http://localhost:8080`
    /// (no trailing slash). When absent it is built from `host`/`port`.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Host (the *arr-pushed shape), e.g. `localhost`.
    #[serde(default)]
    pub host: Option<String>,
    /// Port (the *arr-pushed shape). Defaults to `8080` (the common ruTorrent/
    /// nginx front-end port).
    #[serde(default)]
    pub port: Option<u16>,
    /// The XML-RPC mount path on the front-end. Defaults to `/RPC2`.
    #[serde(rename = "urlBase", default, alias = "url_base")]
    pub url_base: Option<String>,
    /// Optional HTTP Basic username (the web front-end's auth). `None`/empty means
    /// no auth.
    #[serde(default)]
    pub username: Option<String>,
    /// Optional HTTP Basic password.
    #[serde(default)]
    pub password: Option<String>,
    /// Optional absolute download directory the add directs content into (passed
    /// as `d.directory.set`). When unset the daemon's own default is used.
    #[serde(rename = "downloadDir", default, alias = "download_dir")]
    pub download_dir: Option<String>,
}

/// An rTorrent XML-RPC download client.
pub struct RtorrentClient {
    name: String,
    settings: RtorrentSettings,
    category: String,
    transport: Box<dyn HttpTransport>,
}

/// One torrent row projected out of a `d.multicall2` response.
#[derive(Debug, Default, PartialEq)]
struct TorrentRow {
    hash: String,
    /// rTorrent's `d.complete`: `1` once the download has finished.
    complete: bool,
    /// `d.ratio` is a permille integer (1000 == ratio 1.0).
    ratio_permille: i64,
    /// `d.base_path`: the on-disk path of the content (set once it exists).
    base_path: String,
    name: String,
    /// `d.custom1`: the ruTorrent label cellarr files under.
    label: String,
    /// `d.message`: a non-empty tracker/IO message is rTorrent's error channel.
    message: String,
    /// Connected peers (`d.peers_complete` + `d.peers_accounted`).
    peers: i64,
    /// `d.is_active`: whether the torrent is started (vs stopped/queued).
    active: bool,
}

impl RtorrentClient {
    /// Build a client over the production HTTP transport.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        settings: RtorrentSettings,
        category: impl Into<String>,
    ) -> Self {
        Self::with_transport(
            name,
            settings,
            category,
            Box::new(crate::http::ReqwestTransport::new()),
        )
    }

    /// Build a client over a caller-supplied transport (the test seam).
    #[must_use]
    pub fn with_transport(
        name: impl Into<String>,
        settings: RtorrentSettings,
        category: impl Into<String>,
        transport: Box<dyn HttpTransport>,
    ) -> Self {
        Self {
            name: name.into(),
            settings,
            category: category.into(),
            transport,
        }
    }

    /// A human-facing name for logs and the UI.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The category (rTorrent `d.custom1` label) cellarr files its torrents under.
    #[must_use]
    pub fn category(&self) -> &str {
        &self.category
    }

    /// The scheme+host(+port) origin, no trailing slash. Prefers `base_url`, else
    /// assembles `http://<host>:<port>`, defaulting the port to `8080`.
    fn base(&self) -> String {
        if let Some(b) = self
            .settings
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
        {
            return b.trim_end_matches('/').to_string();
        }
        let host = self.settings.host.as_deref().unwrap_or("localhost").trim();
        let host = host.trim_end_matches('/');
        let port = self.settings.port.unwrap_or(8080);
        if host.contains("://") {
            format!("{host}:{port}")
        } else {
            format!("http://{host}:{port}")
        }
    }

    /// The full XML-RPC endpoint URL, honouring the configured mount path
    /// (default `/RPC2`).
    fn rpc_url(&self) -> String {
        let path = self
            .settings
            .url_base
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| {
                if p.starts_with('/') {
                    p.to_string()
                } else {
                    format!("/{p}")
                }
            })
            .unwrap_or_else(|| "/RPC2".to_string());
        format!("{}{}", self.base(), path)
    }

    /// The `Authorization: Basic …` header value when a username is configured.
    fn basic_auth(&self) -> Option<String> {
        let user = self
            .settings
            .username
            .as_deref()
            .filter(|u| !u.is_empty())?;
        let pass = self.settings.password.as_deref().unwrap_or("");
        let raw = format!("{user}:{pass}");
        Some(format!(
            "Basic {}",
            crate::transmission::base64_encode(raw.as_bytes())
        ))
    }

    /// Send one XML-RPC `methodCall` and return the decoded response body.
    ///
    /// Maps a `401` to [`DownloadError::Auth`] and an XML-RPC `<fault>` to
    /// [`DownloadError::Api`].
    async fn call(&self, method: &str, params: &[XmlValue]) -> Result<String, DownloadError> {
        let body = build_method_call(method, params);
        let mut req = HttpRequest::new("POST", self.rpc_url())
            .header("Content-Type", "text/xml")
            .body(body);
        if let Some(auth) = self.basic_auth() {
            req = req.header("Authorization", auth);
        }
        let resp = self.transport.send(req).await?;
        if resp.status == 401 || resp.status == 403 {
            return Err(DownloadError::Auth(format!(
                "rTorrent front-end rejected credentials ({})",
                resp.status
            )));
        }
        if !resp.is_success() {
            return Err(DownloadError::Api(format!(
                "{method} returned status {}",
                resp.status
            )));
        }
        if let Some(fault) = fault_message(&resp.body) {
            return Err(DownloadError::Api(format!("{method}: {fault}")));
        }
        Ok(resp.body)
    }

    /// Add a torrent and return its infohash (the rTorrent download id).
    ///
    /// cellarr resolves the release's `download_url` to a self-contained
    /// [`TorrentSource`] first (fetching the indexer URL itself) so rTorrent never
    /// has to reach the indexer. A magnet is loaded via `load.start`; a `.torrent`
    /// is loaded as raw base64 metainfo via `load.raw_start`. rTorrent does not
    /// return the hash, so cellarr derives the infohash from the source and uses it
    /// to stamp the label (`d.custom1.set`) and an optional `d.directory.set`.
    pub async fn add(&self, grab: &GrabRequest) -> Result<String, DownloadError> {
        let source =
            TorrentSource::resolve(&grab.release.download_url, self.transport.as_ref()).await?;

        let infohash = match &source {
            TorrentSource::Magnet(magnet) => infohash_from_url(magnet).ok_or_else(|| {
                DownloadError::UnexpectedResponse("resolved magnet carries no btih infohash".into())
            })?,
            TorrentSource::Metainfo(bytes) => crate::qbittorrent::infohash_from_metainfo(bytes)
                .ok_or_else(|| {
                    DownloadError::UnexpectedResponse(
                        "could not compute infohash from .torrent metainfo".into(),
                    )
                })?,
        };

        // Load the torrent. rTorrent's load.start signature is
        // `load.start("", <uri>)`; the empty first arg is the (unused) target.
        match &source {
            TorrentSource::Magnet(magnet) => {
                self.call(
                    "load.start",
                    &[XmlValue::Str(String::new()), XmlValue::Str(magnet.clone())],
                )
                .await?;
            }
            TorrentSource::Metainfo(bytes) => {
                self.call(
                    "load.raw_start",
                    &[
                        XmlValue::Str(String::new()),
                        XmlValue::Base64(bytes.clone()),
                    ],
                )
                .await?;
            }
        }

        // Stamp cellarr's label into d.custom1 so the torrent is scoped. The label
        // value is stored verbatim (ruTorrent reads d.custom1 as the label).
        self.call(
            "d.custom1.set",
            &[
                XmlValue::Str(infohash.clone()),
                XmlValue::Str(self.category.clone()),
            ],
        )
        .await?;

        // Direct content into an absolute download dir when configured.
        if let Some(dir) = self.download_dir() {
            self.call(
                "d.directory.set",
                &[XmlValue::Str(infohash.clone()), XmlValue::Str(dir)],
            )
            .await?;
        }

        Ok(infohash)
    }

    /// The absolute download directory to set, or `None` when none is configured.
    fn download_dir(&self) -> Option<String> {
        self.settings
            .download_dir
            .as_deref()
            .map(str::trim)
            .filter(|r| r.starts_with('/'))
            .map(|r| format!("{}/{}", r.trim_end_matches('/'), self.category))
    }

    /// Fetch the `d.multicall2` row for `hash`, or `None` if absent.
    ///
    /// A `d.multicall2` over the `main` view returns every torrent; we select the
    /// row whose `d.hash` matches (case-insensitively). rTorrent has no per-hash
    /// status verb that returns the same column set, so the multicall + filter is
    /// the contract the other tooling uses too.
    async fn fetch_row(&self, hash: &str) -> Result<Option<TorrentRow>, DownloadError> {
        // d.multicall2("", "main", "d.hash=", "d.complete=", ...).
        let params = [
            XmlValue::Str(String::new()),
            XmlValue::Str("main".into()),
            XmlValue::Str("d.hash=".into()),
            XmlValue::Str("d.complete=".into()),
            XmlValue::Str("d.ratio=".into()),
            XmlValue::Str("d.base_path=".into()),
            XmlValue::Str("d.name=".into()),
            XmlValue::Str("d.custom1=".into()),
            XmlValue::Str("d.message=".into()),
            XmlValue::Str("d.peers_complete=".into()),
            XmlValue::Str("d.peers_accounted=".into()),
            XmlValue::Str("d.is_active=".into()),
        ];
        let body = self.call("d.multicall2", &params).await?;
        let rows = parse_multicall_rows(&body);
        Ok(rows.into_iter().find(|r| r.hash.eq_ignore_ascii_case(hash)))
    }

    /// Poll detailed progress for a torrent by infohash.
    pub async fn progress(&self, hash: &str) -> Result<DownloadProgress, DownloadError> {
        let row = self
            .fetch_row(hash)
            .await?
            .ok_or_else(|| DownloadError::NotFound(hash.to_string()))?;
        Ok(progress_from_row(&row))
    }

    /// Poll the coarse [`DownloadState`] of a torrent by infohash.
    pub async fn status(&self, hash: &str) -> Result<DownloadState, DownloadError> {
        Ok(self.progress(hash).await?.state)
    }

    /// Remove a torrent, honouring a ratio/time gate.
    ///
    /// Returns `Ok(false)` without removing when the gate is not yet satisfied, so
    /// the caller can poll again later. Returns `Ok(true)` once erased.
    ///
    /// rTorrent's `d.erase` removes the torrent from the session. Deleting the
    /// downloaded data is **not** a first-class rTorrent RPC (it is conventionally
    /// done by a userscript on the `event.download.erased` hook), so `delete_data`
    /// is honoured at the session level only; see the module note.
    // TODO(deferred): true data deletion needs an rTorrent userscript hook
    // (event.download.erased) or an out-of-band rm of d.base_path; rTorrent exposes
    // no safe single-call RPC for it. Tracked as long-tail; needs a live daemon.
    pub async fn remove(&self, hash: &str, policy: RemovePolicy) -> Result<bool, DownloadError> {
        let progress = self.progress(hash).await?;
        if !policy.is_satisfied_by(&progress) {
            return Ok(false);
        }
        self.call("d.erase", &[XmlValue::Str(hash.to_string())])
            .await?;
        Ok(true)
    }
}

/// Map an rTorrent multicall row to detailed progress.
fn progress_from_row(r: &TorrentRow) -> DownloadProgress {
    // rTorrent's ratio is permille (1000 == 1.0).
    let ratio = r.ratio_permille as f64 / 1000.0;
    let state = if !r.message.is_empty() && is_error_message(&r.message) {
        DownloadState::Failed
    } else if r.complete {
        DownloadState::Completed
    } else if r.active {
        DownloadState::Downloading
    } else {
        // Loaded but not started (stopped/queued): not yet pulling data.
        DownloadState::Queued
    };
    let content_path = if matches!(state, DownloadState::Completed) && !r.base_path.is_empty() {
        // d.base_path is already the full on-disk path of the content.
        Some(r.base_path.clone())
    } else {
        None
    };
    DownloadProgress {
        state,
        progress: if r.complete { 1.0 } else { 0.0 },
        content_path,
        ratio: Some(ratio),
        // rTorrent exposes seed time via timestamps, not a duration column in this
        // set; left None so removal gates on ratio (the common rTorrent setup).
        seeding_time_secs: None,
        peers: Some(r.peers.max(0) as u32),
        error_string: (matches!(state, DownloadState::Failed)).then(|| r.message.clone()),
        category: (!r.label.is_empty()).then(|| r.label.clone()),
    }
}

/// Whether a `d.message` is a genuine error vs an informational tracker line.
///
/// rTorrent puts both transient tracker notices ("Tried all trackers") and hard
/// failures into `d.message`. We treat well-known failure phrasings as errors and
/// otherwise leave the torrent in its progress-derived state, so a transient
/// tracker hiccup does not flip a healthy download to Failed.
fn is_error_message(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("error")
        || lower.contains("failed")
        || lower.contains("unable")
        || lower.contains("denied")
}

/// Extract a BitTorrent v1 infohash from a magnet URI's `xt=urn:btih:` param, or
/// `None` for a non-magnet URL.
fn infohash_from_url(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    let idx = lower.find("xt=urn:btih:")?;
    let after = &url[idx + "xt=urn:btih:".len()..];
    let hash: String = after.chars().take_while(|c| *c != '&').collect();
    if hash.is_empty() {
        None
    } else {
        Some(hash.to_ascii_lowercase())
    }
}

// --- minimal XML-RPC encode/decode -----------------------------------------

/// A typed XML-RPC parameter value the adapter sends.
///
/// rTorrent's surface only needs strings (hashes, URIs, view names, format
/// columns), a base64 blob (raw `.torrent` metainfo), and — internally —
/// integers, so the encoder is deliberately tiny rather than a full XML-RPC crate.
enum XmlValue {
    /// A `<string>` value.
    Str(String),
    /// A `<base64>` value (raw bytes, base64-encoded on the wire).
    Base64(Vec<u8>),
}

/// Build an XML-RPC `methodCall` document for `method` with `params`.
fn build_method_call(method: &str, params: &[XmlValue]) -> String {
    let mut s = String::from("<?xml version=\"1.0\"?><methodCall><methodName>");
    s.push_str(method);
    s.push_str("</methodName><params>");
    for p in params {
        s.push_str("<param><value>");
        match p {
            XmlValue::Str(v) => {
                s.push_str("<string>");
                s.push_str(&xml_escape(v));
                s.push_str("</string>");
            }
            XmlValue::Base64(bytes) => {
                s.push_str("<base64>");
                s.push_str(&crate::transmission::base64_encode(bytes));
                s.push_str("</base64>");
            }
        }
        s.push_str("</value></param>");
    }
    s.push_str("</params></methodCall>");
    s
}

/// XML-escape the five predefined entities for an XML-RPC `<string>` value.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Return the `<fault>` string of an XML-RPC fault response, or `None` when the
/// document is a normal `<methodResponse>`.
fn fault_message(body: &str) -> Option<String> {
    if !body.contains("<fault>") {
        return None;
    }
    // The fault's faultString member is a <string> we surface verbatim. Fall back
    // to a generic message when the exact member cannot be located.
    extract_all_strings(body)
        .into_iter()
        .find(|s| !s.is_empty())
        .or_else(|| Some("rTorrent XML-RPC fault".to_string()))
}

/// Parse a `d.multicall2` response into rows.
///
/// The response is an `<array>` of per-torrent `<array>`s, each carrying the
/// requested columns positionally in the order they were requested:
/// `hash, complete, ratio, base_path, name, custom1, message, peers_complete,
/// peers_accounted, is_active`.
fn parse_multicall_rows(body: &str) -> Vec<TorrentRow> {
    // The outer array's <data> contains one inner <array> per torrent. We split on
    // the inner array boundaries and decode each inner array's <value>s in order.
    let mut rows = Vec::new();
    // Locate each inner array by `<value><array>` … `</array></value>` spans after
    // the outer array's first `<array>`.
    let inner_marker = "<array>";
    // Skip the very first <array> (the outer container).
    let mut search_from = match body.find(inner_marker) {
        Some(idx) => idx + inner_marker.len(),
        None => return rows,
    };
    while let Some(rel) = body[search_from..].find(inner_marker) {
        let start = search_from + rel + inner_marker.len();
        let Some(end_rel) = body[start..].find("</array>") else {
            break;
        };
        let inner = &body[start..start + end_rel];
        rows.push(decode_row(inner));
        search_from = start + end_rel + "</array>".len();
    }
    rows
}

/// Decode one inner-array fragment's ordered `<value>`s into a [`TorrentRow`].
fn decode_row(inner: &str) -> TorrentRow {
    let values = extract_ordered_values(inner);
    let get = |i: usize| values.get(i).cloned().unwrap_or_default();
    let as_i64 = |i: usize| get(i).trim().parse::<i64>().unwrap_or(0);
    let peers_complete = as_i64(7);
    let peers_accounted = as_i64(8);
    TorrentRow {
        hash: get(0),
        complete: as_i64(1) != 0,
        ratio_permille: as_i64(2),
        base_path: get(3),
        name: get(4),
        label: get(5),
        message: get(6),
        peers: peers_complete + peers_accounted,
        active: as_i64(9) != 0,
    }
}

/// Extract each `<value>`'s scalar content in order from an XML-RPC fragment,
/// decoding `<string>`, `<i4>`/`<int>`/`<i8>`, and bare-text values, and
/// unescaping XML entities. This is the minimum needed to read rTorrent's
/// homogeneous multicall rows without a full XML parser.
fn extract_ordered_values(fragment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = fragment;
    while let Some(vs) = rest.find("<value>") {
        let after = &rest[vs + "<value>".len()..];
        let Some(ve) = after.find("</value>") else {
            break;
        };
        let inner = &after[..ve];
        out.push(decode_scalar(inner));
        rest = &after[ve + "</value>".len()..];
    }
    out
}

/// Decode a single XML-RPC scalar `<value>` body to its string form.
fn decode_scalar(inner: &str) -> String {
    for (open, close) in [
        ("<string>", "</string>"),
        ("<i4>", "</i4>"),
        ("<int>", "</int>"),
        ("<i8>", "</i8>"),
        ("<i6>", "</i6>"),
    ] {
        if let Some(s) = between(inner, open, close) {
            return xml_unescape(s);
        }
    }
    // A bare-text <value> (no type tag) — rTorrent uses this for strings.
    xml_unescape(inner.trim())
}

/// Collect every `<string>` value in a document (used for fault extraction).
fn extract_all_strings(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(s) = between(rest, "<string>", "</string>") {
        out.push(xml_unescape(s));
        // Advance past this match.
        if let Some(idx) = rest.find("</string>") {
            rest = &rest[idx + "</string>".len()..];
        } else {
            break;
        }
    }
    out
}

/// The substring between the first `open` and the next `close`, or `None`.
fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = s.find(open)? + open.len();
    let end = s[start..].find(close)? + start;
    Some(&s[start..end])
}

/// Reverse [`xml_escape`] for the five predefined entities.
fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_client(settings: RtorrentSettings, category: &str) -> RtorrentClient {
        struct Dummy;
        #[async_trait::async_trait]
        impl HttpTransport for Dummy {
            async fn send(
                &self,
                _req: HttpRequest,
            ) -> Result<crate::http::HttpResponse, DownloadError> {
                unreachable!("builders make no requests")
            }
        }
        RtorrentClient::with_transport("rtorrent", settings, category, Box::new(Dummy))
    }

    fn base_settings() -> RtorrentSettings {
        RtorrentSettings {
            base_url: None,
            host: None,
            port: None,
            url_base: None,
            username: None,
            password: None,
            download_dir: None,
        }
    }

    #[test]
    fn rpc_url_defaults_to_rpc2() {
        let mut s = base_settings();
        s.host = Some("rt.local".into());
        assert_eq!(dummy_client(s, "c").rpc_url(), "http://rt.local:8080/RPC2");
    }

    #[test]
    fn rpc_url_honors_httprpc_mount() {
        let mut s = base_settings();
        s.base_url = Some("http://localhost".into());
        s.url_base = Some("rutorrent/plugins/httprpc/action.php".into());
        assert_eq!(
            dummy_client(s, "c").rpc_url(),
            "http://localhost/rutorrent/plugins/httprpc/action.php"
        );
    }

    #[test]
    fn basic_auth_present_only_with_username() {
        assert!(dummy_client(base_settings(), "c").basic_auth().is_none());
        let mut s = base_settings();
        s.username = Some("user".into());
        s.password = Some("pw".into());
        assert_eq!(
            dummy_client(s, "c").basic_auth().as_deref(),
            Some("Basic dXNlcjpwdw==")
        );
    }

    #[test]
    fn download_dir_joins_category_when_absolute() {
        let mut s = base_settings();
        s.download_dir = Some("/data/".into());
        assert_eq!(
            dummy_client(s, "cellarr-tv").download_dir().as_deref(),
            Some("/data/cellarr-tv")
        );
        assert_eq!(dummy_client(base_settings(), "c").download_dir(), None);
    }

    #[test]
    fn method_call_encodes_string_and_base64_params() {
        let xml = build_method_call(
            "load.raw_start",
            &[
                XmlValue::Str("".into()),
                XmlValue::Base64(b"BENCODE".to_vec()),
            ],
        );
        assert!(xml.contains("<methodName>load.raw_start</methodName>"));
        assert!(xml.contains("<string></string>"));
        assert!(xml.contains("<base64>QkVOQ09ERQ==</base64>"));
    }

    #[test]
    fn xml_escape_and_unescape_roundtrip() {
        let raw = "a&b<c>d\"e'f";
        assert_eq!(xml_unescape(&xml_escape(raw)), raw);
    }

    #[test]
    fn parse_multicall_rows_decodes_columns_in_order() {
        // One torrent row: hash, complete=1, ratio=2500 (2.5), base_path, name,
        // custom1=label, message="", peers_complete=3, peers_accounted=2, active=1.
        let body = r#"<?xml version="1.0"?><methodResponse><params><param><value>
            <array><data>
              <value><array><data>
                <value><string>DEADBEEFCAFEF00D</string></value>
                <value><i8>1</i8></value>
                <value><i8>2500</i8></value>
                <value><string>/downloads/cellarr-tv/Show.S01E01</string></value>
                <value><string>Show.S01E01</string></value>
                <value><string>cellarr-tv</string></value>
                <value><string></string></value>
                <value><i8>3</i8></value>
                <value><i8>2</i8></value>
                <value><i8>1</i8></value>
              </data></array></value>
            </data></array>
        </value></param></params></methodResponse>"#;
        let rows = parse_multicall_rows(body);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.hash, "DEADBEEFCAFEF00D");
        assert!(r.complete);
        assert_eq!(r.ratio_permille, 2500);
        assert_eq!(r.base_path, "/downloads/cellarr-tv/Show.S01E01");
        assert_eq!(r.label, "cellarr-tv");
        assert_eq!(r.peers, 5);
        assert!(r.active);

        let p = progress_from_row(r);
        assert_eq!(p.state, DownloadState::Completed);
        assert_eq!(
            p.content_path.as_deref(),
            Some("/downloads/cellarr-tv/Show.S01E01")
        );
        assert_eq!(p.ratio, Some(2.5));
        assert_eq!(p.peers, Some(5));
        assert_eq!(p.category.as_deref(), Some("cellarr-tv"));
    }

    #[test]
    fn downloading_row_is_in_flight_without_content_path() {
        let r = TorrentRow {
            hash: "h".into(),
            complete: false,
            ratio_permille: 0,
            base_path: "/downloads/partial".into(),
            name: "x".into(),
            label: String::new(),
            message: String::new(),
            peers: 4,
            active: true,
        };
        let p = progress_from_row(&r);
        assert_eq!(p.state, DownloadState::Downloading);
        assert!(p.content_path.is_none());
    }

    #[test]
    fn error_message_is_failed() {
        let r = TorrentRow {
            hash: "h".into(),
            message: "Download error: file too large".into(),
            ..Default::default()
        };
        let p = progress_from_row(&r);
        assert_eq!(p.state, DownloadState::Failed);
        assert_eq!(
            p.error_string.as_deref(),
            Some("Download error: file too large")
        );
    }

    #[test]
    fn transient_tracker_message_is_not_an_error() {
        let r = TorrentRow {
            hash: "h".into(),
            complete: true,
            message: "Tried all trackers".into(),
            base_path: "/d/x".into(),
            ..Default::default()
        };
        // "Tried all trackers" is not a failure phrasing -> stays completed.
        assert_eq!(progress_from_row(&r).state, DownloadState::Completed);
    }

    #[test]
    fn fault_response_surfaces_string() {
        let body = r#"<?xml version="1.0"?><methodResponse><fault><value><struct>
            <member><name>faultCode</name><value><i4>-506</i4></value></member>
            <member><name>faultString</name><value><string>Method not found</string></value></member>
        </struct></value></fault></methodResponse>"#;
        assert_eq!(fault_message(body).as_deref(), Some("Method not found"));
    }

    #[test]
    fn non_fault_response_has_no_fault() {
        assert!(fault_message("<methodResponse><params/></methodResponse>").is_none());
    }
}
