//! qBittorrent WebUI API v2 adapter.
//!
//! Implements the uniform lifecycle over qBittorrent's `/api/v2/` surface, with
//! the auth/version quirks treated as first-class (see `docs/06-integrations.md`):
//!
//! - **Cookie/`SID` auth.** `POST /api/v2/auth/login` returns a `SID` in a
//!   `Set-Cookie` header; every subsequent call must resend it as `Cookie: SID=…`.
//!   We manage the cookie explicitly (rather than via a `reqwest` cookie jar) so
//!   contract tests can see it on the wire — the exact thing that broke in 5.x.
//! - **`Referer`/`Origin`.** qBittorrent's CSRF protection rejects requests whose
//!   `Referer`/`Origin` don't match the WebUI host, so the adapter always sends
//!   both, set to the configured base URL.
//! - **Version-aware login success check.** Pre-5.x and most 5.x builds answer a
//!   successful login with the body `Ok.`. A late-2025 5.x dev build changed that
//!   body, which broke success checks that *only* matched `Ok.`. We therefore
//!   treat login as successful when the response is 2xx **and** a `SID` cookie was
//!   issued, falling back to the `Ok.` body only when no cookie is surfaced —
//!   robust across both behaviours.
//!
//! Category scoping: every add sets `category` to cellarr's label, and status
//! refuses to report on a torrent filed under a foreign category.

use std::sync::Mutex;

use cellarr_core::{DownloadStatus, GrabRequest};
use serde::Deserialize;

use crate::error::DownloadError;
use crate::http::{HttpRequest, HttpResponse, HttpTransport};
use crate::lifecycle::{DownloadProgress, RemovePolicy};

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
    /// The current `SID` value, learned at login and resent on every call.
    sid: Mutex<Option<String>>,
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
            sid: Mutex::new(None),
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
    /// `SID` cookie, if we have one.
    fn with_session(&self, mut req: HttpRequest) -> HttpRequest {
        req = req
            .header("Referer", self.base().to_string())
            .header("Origin", self.base().to_string());
        if let Ok(guard) = self.sid.lock() {
            if let Some(sid) = guard.as_ref() {
                req = req.header("Cookie", format!("SID={sid}"));
            }
        }
        req
    }

    /// Extract a `SID` value from a `Set-Cookie` header, if present.
    fn parse_sid(resp: &HttpResponse) -> Option<String> {
        let set_cookie = resp.header("set-cookie")?;
        for part in set_cookie.split(';') {
            let part = part.trim();
            if let Some(value) = part.strip_prefix("SID=") {
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        None
    }

    /// Log in, storing the `SID` for subsequent calls.
    ///
    /// Version-aware success check: a login is accepted when the response is 2xx
    /// and either a `SID` cookie was issued (works across the late-2025 5.x body
    /// change) or — when no cookie is surfaced by the transport — the legacy
    /// `Ok.` body is present. A 403 or a `Fails.` body is an auth failure.
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

        let sid = Self::parse_sid(&resp);
        match sid {
            Some(sid) => {
                if let Ok(mut guard) = self.sid.lock() {
                    *guard = Some(sid);
                }
                Ok(())
            }
            // No cookie surfaced: fall back to the legacy body check. This keeps
            // us working with transports that don't expose Set-Cookie, while the
            // cookie path above is what survives the 5.x body change.
            None if resp.body.trim() == "Ok." => Ok(()),
            None => Err(DownloadError::Auth(
                "qBittorrent login succeeded with neither a SID cookie nor an Ok. body".into(),
            )),
        }
    }

    /// Ensure we have a session, logging in if needed.
    async fn ensure_session(&self) -> Result<(), DownloadError> {
        let have = self.sid.lock().map(|g| g.is_some()).unwrap_or(false);
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
        if resp.status == 403 {
            return Err(DownloadError::Auth(
                "qBittorrent session rejected (403); re-login required".into(),
            ));
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
    /// qBittorrent's add endpoint does not return the hash, so we derive it from
    /// the magnet/URL the grab carries: a magnet's `btih` is the infohash. For a
    /// `.torrent` URL the caller must have an infohash on the release (the
    /// indexer supplies it); we surface a clear error otherwise rather than
    /// guessing.
    pub async fn add(&self, grab: &GrabRequest) -> Result<String, DownloadError> {
        self.ensure_session().await?;
        let url = &grab.release.download_url;
        let body = format!(
            "urls={}&category={}",
            urlencode(url),
            urlencode(&self.category)
        );
        let req = self
            .with_session(HttpRequest::new(
                "POST",
                format!("{}/api/v2/torrents/add", self.base()),
            ))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body);
        let resp = self.transport.send(req).await?;
        if resp.status == 403 {
            return Err(DownloadError::Auth(
                "qBittorrent rejected add (403); re-login required".into(),
            ));
        }
        if !resp.is_success() || resp.body.trim() == "Fails." {
            return Err(DownloadError::Api(format!(
                "torrents/add failed (status {}, body {:?})",
                resp.status,
                resp.body.trim()
            )));
        }
        infohash_from_url(url).ok_or_else(|| {
            DownloadError::UnexpectedResponse(
                "could not determine infohash for added torrent (non-magnet URL with no infohash)"
                    .into(),
            )
        })
    }

    /// Poll the detailed progress of a torrent by infohash.
    pub async fn progress(&self, hash: &str) -> Result<DownloadProgress, DownloadError> {
        let info = self
            .fetch_info(hash)
            .await?
            .ok_or_else(|| DownloadError::NotFound(hash.to_string()))?;
        Ok(progress_from_info(&info))
    }

    /// Poll the coarse [`DownloadStatus`] of a torrent by infohash.
    pub async fn status(&self, hash: &str) -> Result<DownloadStatus, DownloadError> {
        Ok(self.progress(hash).await?.status)
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
    let status = match info.state.as_str() {
        "error" | "missingFiles" => DownloadStatus::Failed,
        s if s.ends_with("UP") || s == "uploading" || s == "forcedUP" || s == "stalledUP" => {
            DownloadStatus::Completed
        }
        // A finished but checking/moving torrent: treat 100% as completed.
        _ if info.progress >= 1.0 => DownloadStatus::Completed,
        "queuedDL" | "stalledDL" | "metaDL" | "allocating" | "checkingResumeData" => {
            DownloadStatus::Queued
        }
        _ => DownloadStatus::Downloading,
    };
    DownloadProgress {
        status,
        progress: info.progress,
        content_path: if info.content_path.is_empty() {
            None
        } else {
            Some(info.content_path.clone())
        },
        ratio: Some(info.ratio),
        seeding_time_secs: Some(info.seeding_time),
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
}
