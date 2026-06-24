//! Transmission RPC adapter.
//!
//! Transmission speaks a single JSON-RPC-ish endpoint at
//! `POST {base}/transmission/rpc` (see `docs/06-integrations.md`):
//!
//! - **CSRF handshake.** The first call (any method) is answered `409 Conflict`
//!   with an `X-Transmission-Session-Id` header. Every subsequent request must
//!   echo that id back in the same header, or it too is rejected `409`. The id can
//!   be rotated by the daemon at any time, so a later `409` is not a failure: the
//!   adapter captures the fresh id from the response and retries the request once.
//! - **Optional HTTP Basic auth.** When `rpc-username`/`rpc-password` are
//!   configured the daemon requires `Authorization: Basic …`; we send it whenever
//!   a username is set, and map a `401` to [`DownloadError::Auth`].
//! - **Add** via `torrent-add`, passing the magnet/`.torrent` as `filename`
//!   (Transmission fetches URLs and resolves magnets itself), the per-category
//!   save path as `download-dir`, cellarr's category as a `labels` entry, and the
//!   `paused` flag. The response carries the new torrent's `hashString`, which is
//!   the cellarr download id.
//! - **Status** via `torrent-get` for the requested fields. Transmission's numeric
//!   `status` plus `percentDone` give the lifecycle state; `downloadDir`+`name`
//!   give the importable `content_path`; `uploadRatio`/`secondsSeeding` feed gated
//!   removal; `labels` carries the category for scoping.
//! - **Remove** via `torrent-remove` with `delete-local-data` per the policy.
//!
//! Category scoping: every add tags the torrent with cellarr's label, and status
//! surfaces the torrent's labels so the caller can refuse to act on a foreign one.

use std::sync::Mutex;

use cellarr_core::{DownloadState, GrabRequest};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::DownloadError;
use crate::http::{HttpRequest, HttpTransport};
use crate::lifecycle::{DownloadProgress, RemovePolicy};
use crate::source::TorrentSource;

/// The header Transmission uses to carry the CSRF session id, both on the `409`
/// challenge response and on every authenticated request.
const SESSION_HEADER: &str = "X-Transmission-Session-Id";

/// Connection + auth settings for a Transmission client, deserialized from a
/// [`cellarr_core::DownloadClientConfig`]'s `settings` JSON.
///
/// Two shapes are accepted. The native/test shape provides a ready `base_url`
/// (`http://host:port`). The *arr-pushed shape (what the API shim persists)
/// provides discrete `host` / `port` / `urlBase` fields, which are assembled into
/// the same base URL. `urlBase` is a path prefix the RPC suffix is appended to
/// (e.g. a reverse-proxy mount point).
#[derive(Debug, Clone, Deserialize)]
pub struct TransmissionSettings {
    /// Pre-assembled base URL of the RPC host, e.g. `http://localhost:9091` (no
    /// trailing slash, no `/transmission/rpc` suffix — that is appended). When
    /// absent it is built from `host`/`port`.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Host (the *arr-pushed shape), e.g. `localhost`. With/without a scheme.
    #[serde(default)]
    pub host: Option<String>,
    /// RPC port (the *arr-pushed shape). Defaults to Transmission's `9091`.
    #[serde(default)]
    pub port: Option<u16>,
    /// Optional URL base / path prefix the RPC suffix mounts under.
    #[serde(rename = "urlBase", default, alias = "url_base")]
    pub url_base: Option<String>,
    /// Optional **absolute** download-dir root the per-category subdir is created
    /// under, e.g. `/downloads`. Transmission rejects a relative `download-dir`
    /// (`"download directory path is not absolute"`), so the category alone is
    /// never sent as a path. When unset, the add omits `download-dir` and lets the
    /// daemon use its own configured root; the `labels` entry still scopes the
    /// torrent to cellarr's category.
    #[serde(rename = "downloadDir", default, alias = "download_dir")]
    pub download_dir: Option<String>,
    /// Optional RPC username (HTTP Basic). `None`/empty means no auth.
    #[serde(default)]
    pub username: Option<String>,
    /// Optional RPC password (HTTP Basic).
    #[serde(default)]
    pub password: Option<String>,
}

/// A Transmission RPC download client.
pub struct TransmissionClient {
    name: String,
    settings: TransmissionSettings,
    category: String,
    transport: Box<dyn HttpTransport>,
    /// The current `X-Transmission-Session-Id`, learned from the first `409`
    /// challenge and resent on every call; refreshed when the daemon rotates it.
    session_id: Mutex<Option<String>>,
}

/// One torrent row from `torrent-get`.
#[derive(Debug, Deserialize)]
struct Torrent {
    #[serde(default)]
    name: String,
    /// Fraction complete in `[0.0, 1.0]`.
    #[serde(rename = "percentDone", default)]
    percent_done: f64,
    /// Transmission's numeric status: 0 stopped, 1 check-wait, 2 checking,
    /// 3 download-wait, 4 downloading, 5 seed-wait, 6 seeding.
    #[serde(default)]
    status: i64,
    #[serde(rename = "downloadDir", default)]
    download_dir: String,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(rename = "uploadRatio", default)]
    upload_ratio: f64,
    #[serde(rename = "secondsSeeding", default)]
    seconds_seeding: i64,
    /// Connected peers, for the no-peers stall signal.
    #[serde(rename = "peersConnected", default)]
    peers_connected: i64,
    #[serde(rename = "errorString", default)]
    error_string: String,
}

impl TransmissionClient {
    /// Build a client over the production HTTP transport.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        settings: TransmissionSettings,
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
        settings: TransmissionSettings,
        category: impl Into<String>,
        transport: Box<dyn HttpTransport>,
    ) -> Self {
        Self {
            name: name.into(),
            settings,
            category: category.into(),
            transport,
            session_id: Mutex::new(None),
        }
    }

    /// A human-facing name for logs and the UI.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The category cellarr files its torrents under.
    #[must_use]
    pub fn category(&self) -> &str {
        &self.category
    }

    /// The scheme+host(+port) origin, with no trailing slash. Prefers an explicit
    /// `base_url`; otherwise assembles `http://<host>:<port>` from the discrete
    /// fields, defaulting the port to Transmission's `9091`.
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
        let port = self.settings.port.unwrap_or(9091);
        if host.contains("://") {
            format!("{host}:{port}")
        } else {
            format!("http://{host}:{port}")
        }
    }

    /// The full RPC endpoint URL, honouring an optional `urlBase` path prefix.
    ///
    /// `url_base` is a reverse-proxy path prefix prepended to Transmission's
    /// standard `/transmission/rpc` endpoint (so `/proxy` → `/proxy/transmission/rpc`).
    /// But a user commonly copies Transmission's *own* default rpc-url
    /// (`/transmission` or `/transmission/`) into this field — prepending that
    /// verbatim would yield `/transmission/transmission/rpc` and a 404. So a
    /// `url_base` that is already exactly `transmission` is treated as the standard
    /// endpoint, not doubled.
    fn rpc_url(&self) -> String {
        let prefix = self
            .settings
            .url_base
            .as_deref()
            .map(str::trim)
            .map(|p| p.trim_matches('/'))
            .filter(|p| !p.is_empty() && *p != "transmission")
            .map(|p| format!("/{p}"))
            .unwrap_or_default();
        format!("{}{}/transmission/rpc", self.base(), prefix)
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
        Some(format!("Basic {}", base64_encode(raw.as_bytes())))
    }

    /// Build a request for `payload`, attaching the current session id and basic
    /// auth if configured.
    fn build_request(&self, payload: &Value) -> HttpRequest {
        let mut req = HttpRequest::new("POST", self.rpc_url())
            .header("Content-Type", "application/json")
            .body(payload.to_string());
        if let Some(auth) = self.basic_auth() {
            req = req.header("Authorization", auth);
        }
        if let Ok(guard) = self.session_id.lock() {
            if let Some(id) = guard.as_ref() {
                req = req.header(SESSION_HEADER, id.clone());
            }
        }
        req
    }

    /// Make one RPC call, transparently completing the CSRF handshake.
    ///
    /// On a `409` the response carries a fresh `X-Transmission-Session-Id`; we
    /// capture it and retry the request once. Any further `409` (or a `401`) is a
    /// hard failure. The returned value is the RPC envelope's `arguments` object.
    async fn call(&self, method: &str, arguments: Value) -> Result<Value, DownloadError> {
        let payload = json!({ "method": method, "arguments": arguments });

        // First attempt with whatever session id we currently hold (none on the
        // very first call).
        let mut resp = self.transport.send(self.build_request(&payload)).await?;

        // CSRF challenge / rotation: capture the new id and retry exactly once.
        if resp.status == 409 {
            let id = resp.header(SESSION_HEADER).ok_or_else(|| {
                DownloadError::UnexpectedResponse(format!(
                    "{method}: 409 without {SESSION_HEADER} header"
                ))
            })?;
            if let Ok(mut guard) = self.session_id.lock() {
                *guard = Some(id.to_string());
            }
            resp = self.transport.send(self.build_request(&payload)).await?;
        }

        if resp.status == 401 {
            return Err(DownloadError::Auth(
                "Transmission rejected credentials (401)".into(),
            ));
        }
        if resp.status == 409 {
            return Err(DownloadError::Auth(format!(
                "{method}: still 409 after session-id handshake"
            )));
        }
        if !resp.is_success() {
            return Err(DownloadError::Api(format!(
                "{method} returned status {}",
                resp.status
            )));
        }

        let envelope: Value = serde_json::from_str(&resp.body)
            .map_err(|e| DownloadError::UnexpectedResponse(format!("{method}: {e}")))?;
        // Transmission reports application-level outcome in a top-level `result`
        // string: "success" or an error message.
        match envelope.get("result").and_then(Value::as_str) {
            Some("success") => {}
            Some(other) => {
                return Err(DownloadError::Api(format!("{method}: {other}")));
            }
            None => {
                return Err(DownloadError::UnexpectedResponse(format!(
                    "{method}: response has no result field"
                )));
            }
        }
        Ok(envelope.get("arguments").cloned().unwrap_or(Value::Null))
    }

    /// Add a torrent and return its `hashString` (the cellarr download id).
    ///
    /// cellarr resolves the release's `download_url` to a self-contained
    /// [`TorrentSource`] **first** (fetching the indexer URL itself), so the daemon
    /// never has to reach the indexer — which is unreachable from the daemon's
    /// network when cellarr talks to the indexer over a port-forward/VPN. A magnet
    /// is then passed as `filename` (Transmission resolves magnets via DHT/trackers
    /// on its own); a `.torrent` is passed as base64 `metainfo` (no fetch needed).
    /// The per-category save path is sent as `download-dir` (only when an absolute
    /// root is configured — see [`Self::download_dir`]), cellarr's category as a
    /// `labels` entry, and the `paused` flag verbatim.
    pub async fn add(&self, grab: &GrabRequest, paused: bool) -> Result<String, DownloadError> {
        let source =
            TorrentSource::resolve(&grab.release.download_url, self.transport.as_ref()).await?;
        let mut args = match source {
            TorrentSource::Magnet(magnet) => json!({
                "filename": magnet,
                "labels": [self.category],
                "paused": paused,
            }),
            TorrentSource::Metainfo(bytes) => json!({
                "metainfo": base64_encode(&bytes),
                "labels": [self.category],
                "paused": paused,
            }),
        };
        // Transmission rejects a relative download-dir, so we only set it when we
        // have an absolute per-category path; otherwise the daemon's own root is
        // used and the label still scopes the torrent to cellarr.
        if let Some(dir) = self.download_dir() {
            if let Some(obj) = args.as_object_mut() {
                obj.insert("download-dir".into(), Value::String(dir));
            }
        }
        let arguments = self.call("torrent-add", args).await?;
        // A fresh add returns `torrent-added`; a duplicate returns
        // `torrent-duplicate`. Either carries the hashString we want.
        let added = arguments
            .get("torrent-added")
            .or_else(|| arguments.get("torrent-duplicate"))
            .ok_or_else(|| {
                DownloadError::UnexpectedResponse(
                    "torrent-add: response missing torrent-added/torrent-duplicate".into(),
                )
            })?;
        let hash = added
            .get("hashString")
            .and_then(Value::as_str)
            .filter(|h| !h.is_empty())
            .ok_or_else(|| {
                DownloadError::UnexpectedResponse(
                    "torrent-add: added torrent has no hashString".into(),
                )
            })?;
        Ok(hash.to_string())
    }

    /// The absolute per-category save path to send as `download-dir`, or `None`
    /// when no absolute root is configured.
    ///
    /// Transmission rejects a relative `download-dir`
    /// (`"download directory path is not absolute"`, confirmed against a live
    /// RPC v6.0.1 daemon), so cellarr never sends the bare category as a path.
    /// When a `download_dir` root is configured it is joined with the category to
    /// scope cellarr's content into a per-category subdirectory mirroring the
    /// label; if the configured root is itself relative, or none is set, we omit
    /// `download-dir` entirely and let the daemon use its own configured root
    /// (the `labels` entry still scopes the torrent to cellarr's category).
    fn download_dir(&self) -> Option<String> {
        let root = self
            .settings
            .download_dir
            .as_deref()
            .map(str::trim)
            .filter(|r| r.starts_with('/'))?;
        Some(format!("{}/{}", root.trim_end_matches('/'), self.category))
    }

    /// Fetch the `torrent-get` row for `hash`, or `None` if absent.
    async fn fetch_torrent(&self, hash: &str) -> Result<Option<Torrent>, DownloadError> {
        let args = json!({
            "ids": [hash],
            "fields": [
                "id", "hashString", "name", "percentDone", "status",
                "downloadDir", "labels", "doneDate", "uploadRatio",
                "secondsSeeding", "errorString",
            ],
        });
        let arguments = self.call("torrent-get", args).await?;
        let rows: Vec<Torrent> = serde_json::from_value(
            arguments
                .get("torrents")
                .cloned()
                .unwrap_or(Value::Array(vec![])),
        )
        .map_err(|e| DownloadError::UnexpectedResponse(format!("torrent-get: {e}")))?;
        Ok(rows.into_iter().next())
    }

    /// Poll detailed progress for a torrent by infohash.
    pub async fn progress(&self, hash: &str) -> Result<DownloadProgress, DownloadError> {
        let torrent = self
            .fetch_torrent(hash)
            .await?
            .ok_or_else(|| DownloadError::NotFound(hash.to_string()))?;
        Ok(progress_from_torrent(&torrent))
    }

    /// Poll the coarse [`DownloadState`] of a torrent by infohash.
    pub async fn status(&self, hash: &str) -> Result<DownloadState, DownloadError> {
        Ok(self.progress(hash).await?.state)
    }

    /// Remove a torrent, honouring a ratio/time gate.
    ///
    /// Returns `Ok(false)` without removing when the gate is not yet satisfied, so
    /// the caller can poll again later. Returns `Ok(true)` once removed.
    pub async fn remove(&self, hash: &str, policy: RemovePolicy) -> Result<bool, DownloadError> {
        let progress = self.progress(hash).await?;
        if !policy.is_satisfied_by(&progress) {
            return Ok(false);
        }
        let args = json!({
            "ids": [hash],
            "delete-local-data": policy.delete_data,
        });
        self.call("torrent-remove", args).await?;
        Ok(true)
    }
}

/// Map a Transmission torrent row to detailed progress.
fn progress_from_torrent(t: &Torrent) -> DownloadProgress {
    // A non-empty errorString is a hard failure regardless of status.
    let state = if !t.error_string.is_empty() {
        DownloadState::Failed
    } else if t.percent_done >= 1.0 || matches!(t.status, 5 | 6) {
        // status 5 (seed-wait) / 6 (seeding) — the content is on disk and the
        // torrent has finished; treat 100% the same even mid-status-transition.
        DownloadState::Completed
    } else {
        match t.status {
            // 0 stopped, 1 check-wait, 2 checking, 3 download-wait: not yet
            // actively pulling data.
            0..=3 => DownloadState::Queued,
            // 4 downloading (and any unexpected code) — in flight.
            _ => DownloadState::Downloading,
        }
    };
    let content_path = if matches!(state, DownloadState::Completed)
        && !t.download_dir.is_empty()
        && !t.name.is_empty()
    {
        Some(format!(
            "{}/{}",
            t.download_dir.trim_end_matches('/'),
            t.name
        ))
    } else {
        None
    };
    DownloadProgress {
        state,
        progress: t.percent_done,
        content_path,
        ratio: Some(t.upload_ratio),
        seeding_time_secs: Some(t.seconds_seeding.max(0) as u64),
        peers: Some(t.peers_connected.max(0) as u32),
        error_string: (!t.error_string.is_empty()).then(|| t.error_string.clone()),
        category: t.labels.first().cloned(),
    }
}

/// Standard base64 encoder (for the Basic auth header), avoiding a base64 crate
/// dependency for this single call site.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_path_joins_dir_and_name_when_completed() {
        let t = Torrent {
            name: "Show.S01E01".into(),
            percent_done: 1.0,
            status: 6,
            download_dir: "/downloads/cellarr-tv/".into(),
            labels: vec!["cellarr-tv".into()],
            upload_ratio: 1.0,
            seconds_seeding: 10,
            peers_connected: 5,
            error_string: String::new(),
        };
        let p = progress_from_torrent(&t);
        assert_eq!(p.state, DownloadState::Completed);
        assert_eq!(
            p.content_path.as_deref(),
            Some("/downloads/cellarr-tv/Show.S01E01")
        );
        assert_eq!(p.category.as_deref(), Some("cellarr-tv"));
    }

    #[test]
    fn downloading_has_no_content_path() {
        let t = Torrent {
            name: "Show.S01E01".into(),
            percent_done: 0.4,
            status: 4,
            download_dir: "/downloads".into(),
            labels: vec![],
            upload_ratio: 0.0,
            seconds_seeding: 0,
            peers_connected: 3,
            error_string: String::new(),
        };
        let p = progress_from_torrent(&t);
        assert_eq!(p.state, DownloadState::Downloading);
        assert!(p.content_path.is_none());
        assert!(p.category.is_none());
    }

    #[test]
    fn error_string_is_failed() {
        let t = Torrent {
            name: "x".into(),
            percent_done: 0.2,
            status: 4,
            download_dir: "/d".into(),
            labels: vec![],
            upload_ratio: 0.0,
            seconds_seeding: 0,
            peers_connected: 0,
            error_string: "tracker error".into(),
        };
        assert_eq!(progress_from_torrent(&t).state, DownloadState::Failed);
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(
            base64_encode(b"transmission:secret"),
            "dHJhbnNtaXNzaW9uOnNlY3JldA=="
        );
    }

    fn client(settings: TransmissionSettings, category: &str) -> TransmissionClient {
        // A no-op transport is fine: these tests only exercise URL/path builders.
        struct Dummy;
        #[async_trait::async_trait]
        impl HttpTransport for Dummy {
            async fn send(
                &self,
                _req: HttpRequest,
            ) -> Result<crate::http::HttpResponse, DownloadError> {
                unreachable!("path/url builders make no requests")
            }
        }
        TransmissionClient::with_transport("t", settings, category, Box::new(Dummy))
    }

    fn base_settings() -> TransmissionSettings {
        TransmissionSettings {
            base_url: None,
            host: None,
            port: None,
            url_base: None,
            download_dir: None,
            username: None,
            password: None,
        }
    }

    #[test]
    fn rpc_url_from_host_port_defaults_and_url_base() {
        let mut s = base_settings();
        s.host = Some("transmission.local".into());
        let c = client(s, "cat");
        assert_eq!(
            c.rpc_url(),
            "http://transmission.local:9091/transmission/rpc"
        );

        let mut s = base_settings();
        s.base_url = Some("http://localhost:9091/".into());
        s.url_base = Some("/proxy/".into());
        let c = client(s, "cat");
        assert_eq!(c.rpc_url(), "http://localhost:9091/proxy/transmission/rpc");

        // A url_base of Transmission's own default rpc-url must NOT double the
        // /transmission segment (the live 404 we hit). "/transmission/",
        // "transmission", and "/transmission" all resolve to the standard endpoint.
        for ub in ["/transmission/", "transmission", "/transmission"] {
            let mut s = base_settings();
            s.host = Some("tr.local".into());
            s.url_base = Some(ub.into());
            let c = client(s, "cat");
            assert_eq!(
                c.rpc_url(),
                "http://tr.local:9091/transmission/rpc",
                "url_base {ub:?} must not double /transmission"
            );
        }
    }

    #[test]
    fn download_dir_only_when_absolute_root_configured() {
        // No root: omitted (the daemon's own root is used). A relative root is
        // likewise omitted because Transmission rejects a relative download-dir.
        assert_eq!(client(base_settings(), "cellarr-tv").download_dir(), None);
        let mut s = base_settings();
        s.download_dir = Some("relative/path".into());
        assert_eq!(client(s, "cellarr-tv").download_dir(), None);

        // An absolute root is joined with the category.
        let mut s = base_settings();
        s.download_dir = Some("/downloads/".into());
        assert_eq!(
            client(s, "cellarr-tv").download_dir().as_deref(),
            Some("/downloads/cellarr-tv")
        );
    }
}
