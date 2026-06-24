//! Deluge JSON-RPC (WebUI) adapter.
//!
//! Deluge's WebUI exposes a single JSON-RPC endpoint at `POST {base}/json` (see
//! `docs/06-integrations.md`). Every call is a `{ "method", "params", "id" }`
//! envelope answered with `{ "result", "error", "id" }`. The quirks the adapter
//! treats as first-class:
//!
//! - **Cookie/session auth.** `auth.login(password)` returns `result: true` and a
//!   `Set-Cookie: _session_id=…` header; every subsequent call must resend that
//!   cookie. We manage it explicitly (rather than via a `reqwest` cookie jar) so
//!   contract tests can see it on the wire — the same seam the qBittorrent adapter
//!   uses for its `SID`. A `result: false` from `auth.login` is bad credentials.
//! - **Label plugin for categories.** Deluge models cellarr's category as a
//!   *label* (the `Label` plugin). After an add we call `label.set_torrent(hash,
//!   label)` to file the torrent; the label is read back from
//!   `core.get_torrent_status`'s `label` key for scoping. Setting a label the
//!   plugin does not yet know is tolerated: the add still succeeds, and a missing
//!   label simply leaves the torrent unscoped (it is logged, never fatal — the
//!   plugin may be disabled).
//! - **Add returns the hash.** `core.add_torrent_magnet`/`core.add_torrent_file`
//!   return the torrent's infohash directly (unlike qBittorrent), which is the
//!   cellarr download id. A duplicate add answers `error` with a "already in
//!   session" message carrying no hash, so we fall back to the source's own
//!   infohash in that case.
//! - **Status → lifecycle.** `core.get_torrent_status(hash, keys)` returns
//!   `state` (`Downloading`/`Seeding`/`Paused`/`Error`/`Checking`/…), `progress`
//!   (0–100), `download_location`+`name` (→ `content_path` on completion),
//!   `ratio`, `seeding_time` (seconds), `num_peers`, and `label`.
//! - **Remove** via `core.remove_torrent(hash, remove_data)` per the policy.
//!
//! Category scoping: every add labels the torrent with cellarr's category, and
//! status surfaces the torrent's label so the caller can refuse to act on a
//! foreign one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use cellarr_core::{DownloadState, GrabRequest};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::DownloadError;
use crate::http::{HttpRequest, HttpResponse, HttpTransport};
use crate::lifecycle::{DownloadProgress, RemovePolicy};
use crate::source::TorrentSource;

/// Connection + auth settings for a Deluge client, deserialized from a
/// [`cellarr_core::DownloadClientConfig`]'s `settings` JSON.
///
/// Two shapes are accepted. The native/test shape provides a ready `base_url`
/// (`http://host:port`). The *arr-pushed shape (what the API shim persists)
/// provides discrete `host` / `port` / `urlBase` fields, which are assembled into
/// the same base URL. `urlBase` is a reverse-proxy path prefix the `/json` suffix
/// is appended to.
#[derive(Debug, Clone, Deserialize)]
pub struct DelugeSettings {
    /// Pre-assembled base URL of the WebUI, e.g. `http://localhost:8112` (no
    /// trailing slash, no `/json` suffix — that is appended). When absent it is
    /// built from `host`/`port`.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Host (the *arr-pushed shape), e.g. `localhost`. With/without a scheme.
    #[serde(default)]
    pub host: Option<String>,
    /// WebUI port (the *arr-pushed shape). Defaults to Deluge's `8112`.
    #[serde(default)]
    pub port: Option<u16>,
    /// Optional URL base / path prefix the `/json` endpoint mounts under.
    #[serde(rename = "urlBase", default, alias = "url_base")]
    pub url_base: Option<String>,
    /// WebUI password (the only credential the JSON-RPC `auth.login` takes).
    #[serde(default)]
    pub password: String,
    /// Optional absolute download root the per-category content lands under. When
    /// set it is passed as the add's `download_location` option; when unset the
    /// daemon's own configured location is used.
    #[serde(rename = "downloadDir", default, alias = "download_dir")]
    pub download_dir: Option<String>,
}

/// A Deluge JSON-RPC download client.
pub struct DelugeClient {
    name: String,
    settings: DelugeSettings,
    category: String,
    transport: Box<dyn HttpTransport>,
    /// The current `_session_id` cookie as a ready-to-send `name=value` pair,
    /// learned at login and resent on every call.
    session_cookie: Mutex<Option<String>>,
    /// A monotonic request id for the JSON-RPC `id` field. Deluge echoes it back;
    /// we do not match on it but send a fresh one per call as the protocol expects.
    next_id: AtomicU64,
}

/// One torrent status row from `core.get_torrent_status`.
#[derive(Debug, Default, Deserialize)]
struct TorrentStatus {
    /// Deluge's state string: `Downloading`, `Seeding`, `Paused`, `Error`,
    /// `Checking`, `Queued`, `Allocating`, `Moving`.
    #[serde(default)]
    state: String,
    /// Percentage complete in `[0.0, 100.0]`.
    #[serde(default)]
    progress: f64,
    /// The on-disk directory the content was downloaded into.
    #[serde(default)]
    download_location: String,
    /// The torrent's name (joined with `download_location` for `content_path`).
    #[serde(default)]
    name: String,
    #[serde(default)]
    ratio: f64,
    /// Seeding time in seconds.
    #[serde(default)]
    seeding_time: u64,
    /// Connected peers (seeds + leechers).
    #[serde(default)]
    num_peers: u32,
    /// The Label-plugin label, when the plugin is enabled and the torrent is
    /// labelled. Empty/absent otherwise.
    #[serde(default)]
    label: String,
    /// Deluge's per-torrent error detail, surfaced on the `Error` state.
    #[serde(default)]
    message: String,
}

impl DelugeClient {
    /// Build a client over the production HTTP transport.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        settings: DelugeSettings,
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
        settings: DelugeSettings,
        category: impl Into<String>,
        transport: Box<dyn HttpTransport>,
    ) -> Self {
        Self {
            name: name.into(),
            settings,
            category: category.into(),
            transport,
            session_cookie: Mutex::new(None),
            next_id: AtomicU64::new(1),
        }
    }

    /// A human-facing name for logs and the UI.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The category (Deluge label) cellarr files its torrents under.
    #[must_use]
    pub fn category(&self) -> &str {
        &self.category
    }

    /// The scheme+host(+port) origin, with no trailing slash. Prefers an explicit
    /// `base_url`; otherwise assembles `http://<host>:<port>` from the discrete
    /// fields, defaulting the port to Deluge's `8112`.
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
        let port = self.settings.port.unwrap_or(8112);
        if host.contains("://") {
            format!("{host}:{port}")
        } else {
            format!("http://{host}:{port}")
        }
    }

    /// The full JSON-RPC endpoint URL, honouring an optional `urlBase` prefix.
    fn rpc_url(&self) -> String {
        let prefix = self
            .settings
            .url_base
            .as_deref()
            .map(str::trim)
            .map(|p| p.trim_matches('/'))
            .filter(|p| !p.is_empty())
            .map(|p| format!("/{p}"))
            .unwrap_or_default();
        format!("{}{}/json", self.base(), prefix)
    }

    /// Attach the current session cookie, if we have one.
    fn with_session(&self, mut req: HttpRequest) -> HttpRequest {
        if let Ok(guard) = self.session_cookie.lock() {
            if let Some(cookie) = guard.as_ref() {
                req = req.header("Cookie", cookie.clone());
            }
        }
        req
    }

    /// Extract the `_session_id` cookie as a ready-to-send `name=value` pair from a
    /// `Set-Cookie` header, if present.
    fn parse_session(resp: &HttpResponse) -> Option<String> {
        let set_cookie = resp.header("set-cookie")?;
        for part in set_cookie.split(';') {
            let part = part.trim();
            let Some((name, value)) = part.split_once('=') else {
                continue;
            };
            if name == "_session_id" && !value.is_empty() {
                return Some(format!("{name}={value}"));
            }
        }
        None
    }

    /// Build a JSON-RPC request envelope for `method`/`params`, with the session
    /// cookie attached.
    fn build_request(&self, method: &str, params: Value) -> HttpRequest {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({ "method": method, "params": params, "id": id });
        self.with_session(
            HttpRequest::new("POST", self.rpc_url())
                .header("Content-Type", "application/json")
                .body(body.to_string()),
        )
    }

    /// Log in, storing the `_session_id` cookie for subsequent calls.
    ///
    /// `auth.login(password)` answers `result: true` plus a session cookie on
    /// success; a `result: false` (or a `401`) is bad credentials.
    async fn login(&self) -> Result<(), DownloadError> {
        let req = self.build_request("auth.login", json!([self.settings.password]));
        let resp = self.transport.send(req).await?;
        if resp.status == 401 || resp.status == 403 {
            return Err(DownloadError::Auth(format!(
                "Deluge rejected login ({})",
                resp.status
            )));
        }
        if !resp.is_success() {
            return Err(DownloadError::Auth(format!(
                "Deluge auth.login returned status {}",
                resp.status
            )));
        }
        let envelope: Value = serde_json::from_str(&resp.body)
            .map_err(|e| DownloadError::UnexpectedResponse(format!("auth.login: {e}")))?;
        // A JSON-RPC error or a `result: false` is an authentication failure.
        if !envelope["error"].is_null() {
            return Err(DownloadError::Auth(format!(
                "Deluge auth.login error: {}",
                envelope["error"]
            )));
        }
        if envelope["result"].as_bool() != Some(true) {
            return Err(DownloadError::Auth(
                "Deluge auth.login failed (bad password)".into(),
            ));
        }
        // Capture the session cookie. Deluge always issues one on a successful
        // login; if a transport hides it we still proceed (the result:true above
        // already authenticated this exchange) but warn so it is diagnosable.
        match Self::parse_session(&resp) {
            Some(cookie) => {
                if let Ok(mut guard) = self.session_cookie.lock() {
                    *guard = Some(cookie);
                }
            }
            None => {
                // No cookie surfaced: the `result: true` above already
                // authenticated this exchange. Proceed without a stored cookie
                // (a transport that hides Set-Cookie, e.g. a test replay) rather
                // than failing a successful login.
            }
        }
        Ok(())
    }

    /// Ensure we have a session, logging in if needed.
    async fn ensure_session(&self) -> Result<(), DownloadError> {
        let have = self
            .session_cookie
            .lock()
            .map(|g| g.is_some())
            .unwrap_or(false);
        if !have {
            self.login().await?;
        }
        Ok(())
    }

    /// Make one authenticated JSON-RPC call, returning the `result` value.
    ///
    /// Maps a JSON-RPC `error` object to a typed [`DownloadError::Api`]; a `401`
    /// to [`DownloadError::Auth`] so the caller can re-login.
    async fn call(&self, method: &str, params: Value) -> Result<Value, DownloadError> {
        self.ensure_session().await?;
        let resp = self
            .transport
            .send(self.build_request(method, params))
            .await?;
        if resp.status == 401 || resp.status == 403 {
            return Err(DownloadError::Auth(format!(
                "Deluge session rejected ({resp_status}) on {method}; re-login required",
                resp_status = resp.status
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
        if !envelope["error"].is_null() {
            return Err(DownloadError::Api(format!(
                "{method}: {}",
                envelope["error"]
            )));
        }
        Ok(envelope.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Add a torrent and return its infohash (the Deluge download id).
    ///
    /// cellarr resolves the release's `download_url` to a self-contained
    /// [`TorrentSource`] **first** (fetching the indexer URL itself), so Deluge
    /// never has to reach the indexer — unreachable from the daemon's network when
    /// cellarr talks to the indexer over a port-forward/VPN. A magnet goes through
    /// `core.add_torrent_magnet`; a `.torrent` is uploaded as base64 via
    /// `core.add_torrent_file`. After the add the torrent is filed under cellarr's
    /// label via the Label plugin.
    ///
    /// Deluge returns the infohash directly. A duplicate add answers an `error`
    /// (already in session) carrying no hash; we then fall back to the source's own
    /// infohash so a re-grab is idempotent rather than a hard failure.
    pub async fn add(&self, grab: &GrabRequest) -> Result<String, DownloadError> {
        // Resolve the source FIRST (cellarr fetches the indexer URL itself), before
        // any login: the source fetch goes to the indexer, not Deluge, and the
        // subsequent `call`s lazily log in. This keeps the indexer fetch ahead of
        // the Deluge handshake on the wire.
        let source =
            TorrentSource::resolve(&grab.release.download_url, self.transport.as_ref()).await?;

        // The add options Deluge accepts. We only set download_location when an
        // absolute root is configured; otherwise the daemon uses its own.
        let mut options = serde_json::Map::new();
        if let Some(dir) = self.download_dir() {
            options.insert("download_location".into(), Value::String(dir));
        }
        let options = Value::Object(options);

        let (method, params, fallback_hash) = match &source {
            TorrentSource::Magnet(magnet) => {
                let fallback = infohash_from_url(magnet);
                (
                    "core.add_torrent_magnet",
                    json!([magnet, options]),
                    fallback,
                )
            }
            TorrentSource::Metainfo(bytes) => {
                let fallback = infohash_from_metainfo(bytes);
                // add_torrent_file(filename, filedump_base64, options). The
                // filename is cosmetic (Deluge keys off the decoded metainfo).
                (
                    "core.add_torrent_file",
                    json!(["cellarr.torrent", base64_encode(bytes), options]),
                    fallback,
                )
            }
        };

        // A duplicate add is an `error` carrying no hash; fall back to the source's
        // own infohash so a re-grab is idempotent.
        let hash = match self.call(method, params).await {
            Ok(result) => match result.as_str().filter(|h| !h.is_empty()) {
                Some(h) => h.to_ascii_lowercase(),
                None => fallback_hash.clone().ok_or_else(|| {
                    DownloadError::UnexpectedResponse(
                        "Deluge add returned no infohash and the source carries none".into(),
                    )
                })?,
            },
            Err(DownloadError::Api(msg)) if msg.to_ascii_lowercase().contains("already") => {
                fallback_hash.clone().ok_or_else(|| {
                    DownloadError::Api(format!(
                        "Deluge add (duplicate) with no fallback hash: {msg}"
                    ))
                })?
            }
            Err(e) => return Err(e),
        };

        // File the torrent under cellarr's label. The Label plugin may be disabled,
        // in which case set_torrent errors; that is not fatal — the torrent still
        // downloads, it is simply unscoped — so we swallow the error and continue.
        let _ = self
            .call(
                "label.set_torrent",
                json!([hash, self.category.to_ascii_lowercase()]),
            )
            .await;

        Ok(hash)
    }

    /// The absolute download root to send as `download_location`, or `None` when
    /// no absolute root is configured (then the daemon's own location is used).
    fn download_dir(&self) -> Option<String> {
        let root = self
            .settings
            .download_dir
            .as_deref()
            .map(str::trim)
            .filter(|r| r.starts_with('/'))?;
        Some(root.trim_end_matches('/').to_string())
    }

    /// The status keys we request from `core.get_torrent_status`.
    fn status_keys() -> Value {
        json!([
            "state",
            "progress",
            "download_location",
            "name",
            "ratio",
            "seeding_time",
            "num_peers",
            "label",
            "message",
        ])
    }

    /// Fetch the status row for `hash`, or `None` if absent.
    ///
    /// Deluge answers an unknown hash with an empty object `{}` (not an error), so
    /// an all-default row with no usable fields is treated as not-found.
    async fn fetch_status(&self, hash: &str) -> Result<Option<TorrentStatus>, DownloadError> {
        let result = self
            .call(
                "core.get_torrent_status",
                json!([hash, Self::status_keys()]),
            )
            .await?;
        // An empty object means the torrent is unknown to the daemon.
        if result.as_object().map(|o| o.is_empty()).unwrap_or(true) {
            return Ok(None);
        }
        let status: TorrentStatus = serde_json::from_value(result)
            .map_err(|e| DownloadError::UnexpectedResponse(format!("get_torrent_status: {e}")))?;
        Ok(Some(status))
    }

    /// Poll the detailed progress of a torrent by infohash.
    pub async fn progress(&self, hash: &str) -> Result<DownloadProgress, DownloadError> {
        let status = self
            .fetch_status(hash)
            .await?
            .ok_or_else(|| DownloadError::NotFound(hash.to_string()))?;
        Ok(progress_from_status(&status))
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
        // core.remove_torrent(hash, remove_data) returns true on success.
        self.call("core.remove_torrent", json!([hash, policy.delete_data]))
            .await?;
        Ok(true)
    }
}

/// Map a Deluge status row to detailed progress.
fn progress_from_status(s: &TorrentStatus) -> DownloadProgress {
    // Deluge reports progress as 0–100; normalize to a fraction.
    let progress = (s.progress / 100.0).clamp(0.0, 1.0);
    let state = match s.state.as_str() {
        "Error" => DownloadState::Failed,
        // Seeding (and a finished-but-paused/queued torrent at 100%) means the
        // content is on disk and importable.
        "Seeding" => DownloadState::Completed,
        _ if progress >= 1.0 => DownloadState::Completed,
        "Queued" | "Checking" | "Allocating" | "Moving" | "Paused" => DownloadState::Queued,
        // Downloading and any unexpected state — in flight.
        _ => DownloadState::Downloading,
    };
    let content_path = if matches!(state, DownloadState::Completed)
        && !s.download_location.is_empty()
        && !s.name.is_empty()
    {
        Some(format!(
            "{}/{}",
            s.download_location.trim_end_matches('/'),
            s.name
        ))
    } else {
        None
    };
    DownloadProgress {
        state,
        progress,
        content_path,
        ratio: Some(s.ratio),
        seeding_time_secs: Some(s.seeding_time),
        peers: Some(s.num_peers),
        error_string: (s.state == "Error" && !s.message.is_empty()).then(|| s.message.clone()),
        category: (!s.label.is_empty()).then(|| s.label.clone()),
    }
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

/// Compute the BitTorrent v1 infohash from `.torrent` metainfo bytes, reusing the
/// qBittorrent adapter's bencode/SHA-1 helpers (the infohash derivation is
/// protocol-level, identical across clients).
fn infohash_from_metainfo(metainfo: &[u8]) -> Option<String> {
    crate::qbittorrent::infohash_from_metainfo(metainfo)
}

/// Standard base64 encoder, reusing the transmission adapter's implementation
/// (Deluge's `add_torrent_file` takes the metainfo as a base64 string).
fn base64_encode(input: &[u8]) -> String {
    crate::transmission::base64_encode(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: &str, progress: f64) -> TorrentStatus {
        TorrentStatus {
            state: state.into(),
            progress,
            download_location: "/downloads/cellarr-tv".into(),
            name: "Show.S01E01".into(),
            ratio: 1.5,
            seeding_time: 7200,
            num_peers: 5,
            label: "cellarr-tv".into(),
            message: String::new(),
        }
    }

    #[test]
    fn seeding_is_completed_with_content_path() {
        let p = progress_from_status(&status("Seeding", 100.0));
        assert_eq!(p.state, DownloadState::Completed);
        assert_eq!(
            p.content_path.as_deref(),
            Some("/downloads/cellarr-tv/Show.S01E01")
        );
        assert_eq!(p.category.as_deref(), Some("cellarr-tv"));
        assert_eq!(p.ratio, Some(1.5));
        assert_eq!(p.seeding_time_secs, Some(7200));
    }

    #[test]
    fn downloading_has_no_content_path_and_fractional_progress() {
        let p = progress_from_status(&status("Downloading", 40.0));
        assert_eq!(p.state, DownloadState::Downloading);
        assert!(p.content_path.is_none());
        assert!((p.progress - 0.4).abs() < 1e-9);
    }

    #[test]
    fn error_state_is_failed_and_carries_message() {
        let mut s = status("Error", 12.0);
        s.message = "tracker gave HTTP 410".into();
        let p = progress_from_status(&s);
        assert_eq!(p.state, DownloadState::Failed);
        assert_eq!(p.error_string.as_deref(), Some("tracker gave HTTP 410"));
    }

    #[test]
    fn paused_is_queued() {
        let p = progress_from_status(&status("Paused", 60.0));
        assert_eq!(p.state, DownloadState::Queued);
    }

    #[test]
    fn infohash_extracted_from_magnet() {
        let url = "magnet:?xt=urn:btih:ABCDEF0123456789&dn=Some.Release";
        assert_eq!(infohash_from_url(url).as_deref(), Some("abcdef0123456789"));
    }

    fn dummy_client(settings: DelugeSettings, category: &str) -> DelugeClient {
        struct Dummy;
        #[async_trait::async_trait]
        impl HttpTransport for Dummy {
            async fn send(&self, _req: HttpRequest) -> Result<HttpResponse, DownloadError> {
                unreachable!("url builders make no requests")
            }
        }
        DelugeClient::with_transport("deluge", settings, category, Box::new(Dummy))
    }

    fn base_settings() -> DelugeSettings {
        DelugeSettings {
            base_url: None,
            host: None,
            port: None,
            url_base: None,
            password: String::new(),
            download_dir: None,
        }
    }

    #[test]
    fn rpc_url_from_host_port_default_and_url_base() {
        let mut s = base_settings();
        s.host = Some("deluge.local".into());
        assert_eq!(
            dummy_client(s, "c").rpc_url(),
            "http://deluge.local:8112/json"
        );

        let mut s = base_settings();
        s.base_url = Some("http://localhost:8112/".into());
        s.url_base = Some("/proxy/".into());
        assert_eq!(
            dummy_client(s, "c").rpc_url(),
            "http://localhost:8112/proxy/json"
        );
    }

    #[test]
    fn download_dir_only_when_absolute() {
        assert_eq!(dummy_client(base_settings(), "c").download_dir(), None);
        let mut s = base_settings();
        s.download_dir = Some("relative".into());
        assert_eq!(dummy_client(s, "c").download_dir(), None);
        let mut s = base_settings();
        s.download_dir = Some("/downloads/".into());
        assert_eq!(
            dummy_client(s, "c").download_dir().as_deref(),
            Some("/downloads")
        );
    }

    #[test]
    fn parse_session_extracts_session_id_pair() {
        let mut headers = std::collections::BTreeMap::new();
        headers.insert(
            "set-cookie".into(),
            "_session_id=abc123def; Path=/; HttpOnly".into(),
        );
        let resp = HttpResponse {
            status: 200,
            headers,
            body: String::new(),
        };
        assert_eq!(
            DelugeClient::parse_session(&resp).as_deref(),
            Some("_session_id=abc123def")
        );
    }
}
