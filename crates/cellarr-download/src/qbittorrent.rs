//! qBittorrent WebUI API v2 adapter.
//!
//! Implements the uniform lifecycle over qBittorrent's `/api/v2/` surface, with
//! the auth/version quirks treated as first-class (see `docs/06-integrations.md`):
//!
//! - **Cookie/session auth.** `POST /api/v2/auth/login` returns a session cookie
//!   in a `Set-Cookie` header; every subsequent call must resend it. We manage the
//!   cookie explicitly (rather than via a `reqwest` cookie jar) so contract tests
//!   can see it on the wire — the exact thing that broke in 5.x. **The cookie name
//!   is version-divergent:** pre-5.x issues `SID`, while qBittorrent 5.x renamed it
//!   to `QBT_SID` / `QBT_SID_<port>`. We therefore capture whichever session cookie
//!   the server sets (name and value) and resend it verbatim, rather than assuming
//!   the name is `SID`.
//! - **`Referer`/`Origin`.** qBittorrent's CSRF protection rejects requests whose
//!   `Referer`/`Origin` don't match the WebUI host, so the adapter always sends
//!   both, set to the configured base URL.
//! - **Version-aware login success check.** Pre-5.x and most 5.x builds answer a
//!   successful login with `200 Ok.`. qBittorrent 5.x instead answers `204 No
//!   Content` with the session cookie and an empty body, which broke success
//!   checks that *only* matched the `Ok.` body. We therefore treat login as
//!   successful when the response is 2xx **and** a session cookie was issued,
//!   falling back to the `Ok.` body only when no cookie is surfaced — robust across
//!   `200 Ok.` and `204`-plus-cookie behaviours alike.
//!
//! Category scoping: every add sets `category` to cellarr's label, and status
//! refuses to report on a torrent filed under a foreign category.

use std::sync::Mutex;

use cellarr_core::{DownloadState, GrabRequest};
use serde::Deserialize;

use crate::error::DownloadError;
use crate::http::{HttpRequest, HttpResponse, HttpTransport};
use crate::lifecycle::{DownloadProgress, RemovePolicy};
use crate::source::TorrentSource;

/// Connection + auth settings for a qBittorrent client, deserialized from a
/// [`cellarr_core::DownloadClientConfig`]'s `settings` JSON.
#[derive(Debug, Clone, Deserialize)]
pub struct QbittorrentSettings {
    /// Base URL of the WebUI, e.g. `http://localhost:8080` (no trailing slash).
    pub base_url: String,
    /// WebUI username.
    pub username: String,
    /// WebUI password.
    pub password: String,
}

/// A qBittorrent WebUI v2 download client.
pub struct QbittorrentClient {
    name: String,
    settings: QbittorrentSettings,
    category: String,
    transport: Box<dyn HttpTransport>,
    /// The current session cookie as a ready-to-send `name=value` pair, learned at
    /// login and resent on every call. The name is version-divergent (`SID` pre-5.x,
    /// `QBT_SID`/`QBT_SID_<port>` on 5.x), so we keep the whole pair rather than
    /// reconstructing `SID=…`.
    session_cookie: Mutex<Option<String>>,
}

/// One torrent row from `GET /api/v2/torrents/info`.
#[derive(Debug, Deserialize)]
struct TorrentInfo {
    /// qBittorrent's lowercase state string (e.g. `downloading`, `uploading`,
    /// `pausedUP`, `error`, `checkingUP`, `stalledUP`).
    state: String,
    /// Save path / on-disk location of the content.
    #[serde(default)]
    content_path: String,
    #[serde(default)]
    progress: f64,
    #[serde(default)]
    ratio: f64,
    #[serde(default)]
    seeding_time: u64,
    /// Connected seeds, for the no-peers stall signal.
    #[serde(default)]
    num_seeds: u32,
    /// Connected leechers, for the no-peers stall signal.
    #[serde(default)]
    num_leechs: u32,
    #[serde(default)]
    category: String,
}

impl QbittorrentClient {
    /// Build a client over the production HTTP transport.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        settings: QbittorrentSettings,
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
        settings: QbittorrentSettings,
        category: impl Into<String>,
        transport: Box<dyn HttpTransport>,
    ) -> Self {
        Self {
            name: name.into(),
            settings,
            category: category.into(),
            transport,
            session_cookie: Mutex::new(None),
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

    fn base(&self) -> &str {
        self.settings.base_url.trim_end_matches('/')
    }

    /// Apply the CSRF-defeating `Referer`/`Origin` headers plus the current
    /// session cookie, if we have one.
    fn with_session(&self, mut req: HttpRequest) -> HttpRequest {
        req = req
            .header("Referer", self.base().to_string())
            .header("Origin", self.base().to_string());
        if let Ok(guard) = self.session_cookie.lock() {
            if let Some(cookie) = guard.as_ref() {
                req = req.header("Cookie", cookie.clone());
            }
        }
        req
    }

    /// Extract the session cookie as a ready-to-send `name=value` pair from a
    /// `Set-Cookie` header, if present.
    ///
    /// Accepts the pre-5.x `SID` cookie and the 5.x `QBT_SID` / `QBT_SID_<port>`
    /// cookies — anything whose name is `SID` or begins with `QBT_SID` — and
    /// returns the literal pair so the caller can resend it verbatim without
    /// hard-coding the (version-divergent) cookie name.
    fn parse_sid(resp: &HttpResponse) -> Option<String> {
        let set_cookie = resp.header("set-cookie")?;
        for part in set_cookie.split(';') {
            let part = part.trim();
            let Some((name, value)) = part.split_once('=') else {
                continue;
            };
            let is_session = name == "SID" || name.starts_with("QBT_SID");
            if is_session && !value.is_empty() {
                return Some(format!("{name}={value}"));
            }
        }
        None
    }

    /// Log in, storing the `SID` for subsequent calls.
    ///
    /// Version-aware success check: a login is accepted when the response is 2xx
    /// and either a session cookie was issued — which covers both `200 Ok.` and the
    /// 5.x `204 No Content` flow — or, when no cookie is surfaced by the transport,
    /// the legacy `Ok.` body is present. A 401/403 or a `Fails.` body is an auth
    /// failure. (qBittorrent 5.x answers bad credentials with `401 Unauthorized`,
    /// where pre-5.x answered `200 Fails.`.)
    async fn login(&self) -> Result<(), DownloadError> {
        let body = format!(
            "username={}&password={}",
            urlencode(&self.settings.username),
            urlencode(&self.settings.password)
        );
        let req = HttpRequest::new("POST", format!("{}/api/v2/auth/login", self.base()))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Referer", self.base().to_string())
            .header("Origin", self.base().to_string())
            .body(body);
        let resp = self.transport.send(req).await?;

        if resp.status == 401 {
            return Err(DownloadError::Auth(
                "qBittorrent login failed (401 — bad username/password on 5.x)".into(),
            ));
        }
        if resp.status == 403 {
            return Err(DownloadError::Auth(
                "qBittorrent rejected login (403 — banned or bad Referer/Origin)".into(),
            ));
        }
        if !resp.is_success() {
            return Err(DownloadError::Auth(format!(
                "qBittorrent login returned status {}",
                resp.status
            )));
        }
        if resp.body.trim() == "Fails." {
            return Err(DownloadError::Auth(
                "qBittorrent login failed (bad username/password)".into(),
            ));
        }

        match Self::parse_sid(&resp) {
            Some(cookie) => {
                if let Ok(mut guard) = self.session_cookie.lock() {
                    *guard = Some(cookie);
                }
                Ok(())
            }
            // No cookie surfaced: fall back to the legacy body check. This keeps
            // us working with transports that don't expose Set-Cookie, while the
            // cookie path above is what survives the 5.x cookie-name/body change.
            None if resp.body.trim() == "Ok." => Ok(()),
            None => Err(DownloadError::Auth(
                "qBittorrent login succeeded with neither a session cookie nor an Ok. body".into(),
            )),
        }
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

    /// Fetch the torrent info row for `hash`, or `None` if absent.
    async fn fetch_info(&self, hash: &str) -> Result<Option<TorrentInfo>, DownloadError> {
        self.ensure_session().await?;
        let url = format!("{}/api/v2/torrents/info?hashes={}", self.base(), hash);
        let req = self.with_session(HttpRequest::new("GET", url));
        let resp = self.transport.send(req).await?;
        if resp.status == 401 || resp.status == 403 {
            return Err(DownloadError::Auth(format!(
                "qBittorrent session rejected ({}); re-login required",
                resp.status
            )));
        }
        if !resp.is_success() {
            return Err(DownloadError::Api(format!(
                "torrents/info returned status {}",
                resp.status
            )));
        }
        let mut rows: Vec<TorrentInfo> = serde_json::from_str(&resp.body)
            .map_err(|e| DownloadError::UnexpectedResponse(format!("torrents/info: {e}")))?;
        Ok(rows.pop())
    }

    /// Add a torrent and return its infohash (the qBittorrent download id).
    ///
    /// cellarr resolves the release's `download_url` to a self-contained
    /// [`TorrentSource`] **first** (fetching the indexer URL itself), so qBittorrent
    /// never has to reach the indexer — which is unreachable from qBittorrent's
    /// network when cellarr talks to the indexer over a port-forward/VPN. A magnet
    /// is submitted via the form `urls=` field; a `.torrent` is uploaded directly as
    /// the multipart `torrents` file part.
    ///
    /// qBittorrent's add endpoint does not return the hash, so we derive the
    /// infohash from the resolved source: a magnet's `btih`, or the SHA-1 of the
    /// uploaded `.torrent`'s bencoded `info` dictionary (the BitTorrent v1 infohash).
    pub async fn add(&self, grab: &GrabRequest) -> Result<String, DownloadError> {
        self.ensure_session().await?;
        let source =
            TorrentSource::resolve(&grab.release.download_url, self.transport.as_ref()).await?;

        let (req, infohash) = match &source {
            TorrentSource::Magnet(magnet) => {
                let body = format!(
                    "urls={}&category={}",
                    urlencode(magnet),
                    urlencode(&self.category)
                );
                let req = self
                    .with_session(HttpRequest::new(
                        "POST",
                        format!("{}/api/v2/torrents/add", self.base()),
                    ))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(body);
                let infohash = infohash_from_url(magnet).ok_or_else(|| {
                    DownloadError::UnexpectedResponse(
                        "resolved magnet carries no btih infohash".into(),
                    )
                })?;
                (req, infohash)
            }
            TorrentSource::Metainfo(bytes) => {
                // Upload the .torrent bytes as a multipart `torrents` file part and
                // a `category` text part. qBittorrent does not echo the hash, so we
                // compute the v1 infohash from the file's bencoded `info` dict.
                let infohash = infohash_from_metainfo(bytes).ok_or_else(|| {
                    DownloadError::UnexpectedResponse(
                        "could not compute infohash from .torrent metainfo (no bencoded info dict)"
                            .into(),
                    )
                })?;
                let (body, content_type) = multipart_add_body(bytes, &self.category);
                let req = self
                    .with_session(HttpRequest::new(
                        "POST",
                        format!("{}/api/v2/torrents/add", self.base()),
                    ))
                    .header("Content-Type", content_type)
                    .body_bytes(body);
                (req, infohash)
            }
        };

        let resp = self.transport.send(req).await?;
        if resp.status == 401 || resp.status == 403 {
            return Err(DownloadError::Auth(format!(
                "qBittorrent rejected add ({}); re-login required",
                resp.status
            )));
        }
        if !resp.is_success() || resp.body.trim() == "Fails." {
            return Err(DownloadError::Api(format!(
                "torrents/add failed (status {}, body {:?})",
                resp.status,
                resp.body.trim()
            )));
        }
        Ok(infohash)
    }

    /// Read the qBittorrent application version (e.g. `v5.1.2`) via
    /// `GET /api/v2/app/version`.
    ///
    /// Used for version/quirk detection and surfaced to the UI. Logs in first so
    /// the call is authenticated (the localhost auth-bypass only covers loopback,
    /// not LAN/container callers — see `docs/06-integrations.md`).
    pub async fn version(&self) -> Result<String, DownloadError> {
        self.ensure_session().await?;
        let req = self.with_session(HttpRequest::new(
            "GET",
            format!("{}/api/v2/app/version", self.base()),
        ));
        let resp = self.transport.send(req).await?;
        if resp.status == 401 || resp.status == 403 {
            return Err(DownloadError::Auth(format!(
                "qBittorrent session rejected ({}) on app/version",
                resp.status
            )));
        }
        if !resp.is_success() {
            return Err(DownloadError::Api(format!(
                "app/version returned status {}",
                resp.status
            )));
        }
        Ok(resp.body.trim().to_string())
    }

    /// Move a torrent into a category via `POST /api/v2/torrents/setCategory`.
    ///
    /// Used to (re-)file a download under cellarr's label — e.g. to claim a
    /// torrent that was added without a category, or to re-scope one. The
    /// endpoint answers `409 Conflict` when the category does not yet exist; we
    /// surface that as an [`DownloadError::Api`] so the caller can create the
    /// category first rather than silently failing.
    pub async fn set_category(&self, hash: &str, category: &str) -> Result<(), DownloadError> {
        self.ensure_session().await?;
        let body = format!(
            "hashes={}&category={}",
            urlencode(hash),
            urlencode(category)
        );
        let req = self
            .with_session(HttpRequest::new(
                "POST",
                format!("{}/api/v2/torrents/setCategory", self.base()),
            ))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body);
        let resp = self.transport.send(req).await?;
        if resp.status == 401 || resp.status == 403 {
            return Err(DownloadError::Auth(format!(
                "qBittorrent rejected setCategory ({}); re-login required",
                resp.status
            )));
        }
        if resp.status == 409 {
            return Err(DownloadError::Api(format!(
                "torrents/setCategory: category {category:?} does not exist (409)"
            )));
        }
        if !resp.is_success() {
            return Err(DownloadError::Api(format!(
                "torrents/setCategory returned status {}",
                resp.status
            )));
        }
        Ok(())
    }

    /// Poll the detailed progress of a torrent by infohash.
    pub async fn progress(&self, hash: &str) -> Result<DownloadProgress, DownloadError> {
        let info = self
            .fetch_info(hash)
            .await?
            .ok_or_else(|| DownloadError::NotFound(hash.to_string()))?;
        Ok(progress_from_info(&info))
    }

    /// Poll the coarse [`DownloadState`] of a torrent by infohash.
    pub async fn status(&self, hash: &str) -> Result<DownloadState, DownloadError> {
        Ok(self.progress(hash).await?.state)
    }

    /// Remove a torrent, honouring a ratio/time gate.
    ///
    /// Returns `Ok(false)` without removing when the gate is not yet satisfied,
    /// so the caller can poll again later. Returns `Ok(true)` once removed.
    pub async fn remove(&self, hash: &str, policy: RemovePolicy) -> Result<bool, DownloadError> {
        let progress = self.progress(hash).await?;
        if !policy.is_satisfied_by(&progress) {
            return Ok(false);
        }
        self.ensure_session().await?;
        let body = format!(
            "hashes={}&deleteFiles={}",
            urlencode(hash),
            policy.delete_data
        );
        let req = self
            .with_session(HttpRequest::new(
                "POST",
                format!("{}/api/v2/torrents/delete", self.base()),
            ))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body);
        let resp = self.transport.send(req).await?;
        if !resp.is_success() {
            return Err(DownloadError::Api(format!(
                "torrents/delete returned status {}",
                resp.status
            )));
        }
        Ok(true)
    }
}

/// Map a qBittorrent state string to detailed progress.
fn progress_from_info(info: &TorrentInfo) -> DownloadProgress {
    // qBittorrent's `error`/`missingFiles` states are terminal failures; any
    // `*UP` / uploading / forced-up / queued-up state means the content is on
    // disk and seeding, i.e. complete and importable.
    let state = match info.state.as_str() {
        "error" | "missingFiles" => DownloadState::Failed,
        s if s.ends_with("UP") || s == "uploading" || s == "forcedUP" || s == "stalledUP" => {
            DownloadState::Completed
        }
        // A finished but checking/moving torrent: treat 100% as completed.
        _ if info.progress >= 1.0 => DownloadState::Completed,
        "queuedDL" | "stalledDL" | "metaDL" | "allocating" | "checkingResumeData" => {
            DownloadState::Queued
        }
        _ => DownloadState::Downloading,
    };
    DownloadProgress {
        state,
        progress: info.progress,
        content_path: if info.content_path.is_empty() {
            None
        } else {
            Some(info.content_path.clone())
        },
        ratio: Some(info.ratio),
        seeding_time_secs: Some(info.seeding_time),
        peers: Some(info.num_seeds + info.num_leechs),
        error_string: None,
        category: if info.category.is_empty() {
            None
        } else {
            Some(info.category.clone())
        },
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

/// A fixed multipart boundary for the `.torrent` upload. A constant is safe here
/// because the only payload is opaque bencoded bytes plus the cellarr category,
/// neither of which contains this token.
const MULTIPART_BOUNDARY: &str = "cellarrFormBoundary7MA4YWxkTrZu0gW";

/// Build the `multipart/form-data` body for a `.torrent` upload: a `torrents`
/// file part carrying the metainfo bytes and a `category` text part. Returns the
/// raw body bytes and the matching `Content-Type` header value.
fn multipart_add_body(metainfo: &[u8], category: &str) -> (Vec<u8>, String) {
    let mut body = Vec::new();
    let dashes = format!("--{MULTIPART_BOUNDARY}\r\n");

    // The .torrent file part (binary, sent verbatim).
    body.extend_from_slice(dashes.as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"torrents\"; filename=\"cellarr.torrent\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: application/x-bittorrent\r\n\r\n");
    body.extend_from_slice(metainfo);
    body.extend_from_slice(b"\r\n");

    // The category text part.
    body.extend_from_slice(dashes.as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"category\"\r\n\r\n");
    body.extend_from_slice(category.as_bytes());
    body.extend_from_slice(b"\r\n");

    body.extend_from_slice(format!("--{MULTIPART_BOUNDARY}--\r\n").as_bytes());

    let content_type = format!("multipart/form-data; boundary={MULTIPART_BOUNDARY}");
    (body, content_type)
}

/// Compute the BitTorrent v1 infohash (lowercase hex SHA-1 of the bencoded `info`
/// dictionary) from `.torrent` metainfo bytes, or `None` if it has no `info` dict.
fn infohash_from_metainfo(metainfo: &[u8]) -> Option<String> {
    let info = bencode_info_slice(metainfo)?;
    Some(hex(&sha1(info)))
}

/// Find the byte slice of the top-level `info` dictionary value inside a bencoded
/// `.torrent` file, so its SHA-1 can be taken as the infohash.
///
/// Parses just enough bencode to walk the top-level dictionary's key/value pairs
/// and return the exact bytes spanning the `info` value (the BitTorrent spec keys
/// the infohash off these verbatim bytes — re-encoding would change them).
fn bencode_info_slice(data: &[u8]) -> Option<&[u8]> {
    let mut pos = 0;
    // The whole file is a dictionary: it must start with 'd'.
    if data.first() != Some(&b'd') {
        return None;
    }
    pos += 1;
    while pos < data.len() && data[pos] != b'e' {
        // A dictionary key is always a bencoded string.
        let (key, after_key) = bencode_string(data, pos)?;
        pos = after_key;
        let value_start = pos;
        pos = bencode_skip(data, pos)?;
        if key == b"info" {
            return data.get(value_start..pos);
        }
    }
    None
}

/// Parse a bencoded string starting at `pos`; return its content and the index
/// just past it.
fn bencode_string(data: &[u8], pos: usize) -> Option<(&[u8], usize)> {
    let colon = data[pos..].iter().position(|&b| b == b':')? + pos;
    let len: usize = std::str::from_utf8(data.get(pos..colon)?)
        .ok()?
        .parse()
        .ok()?;
    let start = colon + 1;
    let end = start.checked_add(len)?;
    let content = data.get(start..end)?;
    Some((content, end))
}

/// Return the index just past the bencoded value starting at `pos` (recursively
/// skipping strings, integers, lists, and dictionaries).
fn bencode_skip(data: &[u8], pos: usize) -> Option<usize> {
    match data.get(pos)? {
        b'i' => {
            // i<digits>e
            let end = data[pos..].iter().position(|&b| b == b'e')? + pos;
            Some(end + 1)
        }
        b'l' | b'd' => {
            // List/dict: skip nested values until the matching 'e'.
            let mut p = pos + 1;
            while *data.get(p)? != b'e' {
                if data[pos] == b'd' {
                    // dict: key (string) then value
                    let (_k, after) = bencode_string(data, p)?;
                    p = bencode_skip(data, after)?;
                } else {
                    p = bencode_skip(data, p)?;
                }
            }
            Some(p + 1)
        }
        b'0'..=b'9' => {
            let (_s, end) = bencode_string(data, pos)?;
            Some(end)
        }
        _ => None,
    }
}

/// Lowercase hex encoding of a byte slice.
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// A dependency-free SHA-1 (RFC 3174) over `data`, returning the 20-byte digest.
///
/// Used solely to derive the BitTorrent v1 infohash from a `.torrent`'s `info`
/// dictionary; pulling a crypto crate for this one non-security use is avoided.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let ml = (data.len() as u64).wrapping_mul(8);

    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&ml.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Minimal `application/x-www-form-urlencoded` value encoder.
///
/// Avoids pulling a URL-encoding crate for the handful of params the adapters
/// send; percent-encodes everything that isn't an unreserved character.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infohash_extracted_from_magnet() {
        let url = "magnet:?xt=urn:btih:ABCDEF0123456789&dn=Some.Release";
        assert_eq!(infohash_from_url(url).as_deref(), Some("abcdef0123456789"));
    }

    #[test]
    fn infohash_none_for_http_torrent() {
        assert!(infohash_from_url("http://idx/dl/x.torrent").is_none());
    }

    #[test]
    fn urlencode_escapes_reserved() {
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
    }

    #[test]
    fn sha1_matches_known_vectors() {
        // RFC 3174 / well-known vectors.
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn infohash_from_metainfo_extracts_and_hashes_info_dict() {
        // A tiny well-formed bencoded torrent; the v1 infohash is the SHA-1 of the
        // verbatim bytes of its `info` value (independently computed in Python).
        let torrent = b"d8:announce13:http://tr/ann4:infod6:lengthi12e4:name9:Some.File12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";
        assert_eq!(
            infohash_from_metainfo(torrent).as_deref(),
            Some("157493ee02747f71737019e994e47f44e5f89b97")
        );
    }

    #[test]
    fn infohash_from_metainfo_none_without_info_dict() {
        assert!(infohash_from_metainfo(b"d8:announce4:noopee").is_none());
    }

    #[test]
    fn multipart_body_frames_file_and_category_parts() {
        let (body, content_type) = multipart_add_body(b"BENCODE", "cellarr-tv");
        let text = String::from_utf8_lossy(&body);
        assert!(content_type.starts_with("multipart/form-data; boundary="));
        assert!(text.contains("name=\"torrents\"; filename=\"cellarr.torrent\""));
        assert!(text.contains("application/x-bittorrent"));
        assert!(text.contains("BENCODE"));
        assert!(text.contains("name=\"category\""));
        assert!(text.contains("cellarr-tv"));
        assert!(text.trim_end().ends_with("--"));
    }

    fn resp_with_set_cookie(set_cookie: &str) -> HttpResponse {
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("set-cookie".into(), set_cookie.into());
        HttpResponse {
            status: 200,
            headers,
            body: String::new(),
        }
    }

    #[test]
    fn parses_legacy_sid_cookie_as_pair() {
        let resp = resp_with_set_cookie("SID=abc123; HttpOnly; path=/");
        assert_eq!(
            QbittorrentClient::parse_sid(&resp).as_deref(),
            Some("SID=abc123")
        );
    }

    #[test]
    fn parses_5x_qbt_sid_port_cookie_as_pair() {
        // qBittorrent 5.x renamed the session cookie to QBT_SID_<port>; the value
        // can itself contain '=' / '+' (base64), so we keep the literal pair.
        let resp = resp_with_set_cookie(
            "QBT_SID_8080=sGPxtCf2VEb8P6+qDMSfu2RME/t90o7p; HttpOnly; SameSite=Lax; path=/",
        );
        assert_eq!(
            QbittorrentClient::parse_sid(&resp).as_deref(),
            Some("QBT_SID_8080=sGPxtCf2VEb8P6+qDMSfu2RME/t90o7p")
        );
    }

    #[test]
    fn parse_sid_ignores_attribute_only_parts() {
        // A Set-Cookie whose first pair is not the session cookie, plus valueless
        // attributes, must not short-circuit the scan.
        let resp = resp_with_set_cookie("Other=x; Secure; QBT_SID=zzz; HttpOnly");
        assert_eq!(
            QbittorrentClient::parse_sid(&resp).as_deref(),
            Some("QBT_SID=zzz")
        );
    }

    #[test]
    fn parse_sid_none_when_no_session_cookie() {
        let resp = resp_with_set_cookie("foo=bar; HttpOnly");
        assert!(QbittorrentClient::parse_sid(&resp).is_none());
    }
}
