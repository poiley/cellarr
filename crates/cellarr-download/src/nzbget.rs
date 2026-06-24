//! NZBGet adapter.
//!
//! NZBGet speaks JSON-RPC 2.0 over HTTP at `/jsonrpc`, with **positional**
//! params and HTTP Basic auth (see `docs/06-integrations.md`):
//!
//! - **Add** via `append`, passing the category as the NZB's category so cellarr
//!   only touches its own downloads. `append` returns the new NZB id (an `i64`);
//!   `0`/negative means the add was rejected.
//! - **Poll** via `listgroups` (active queue) then `history` (finished). NZBGet
//!   does post-processing (par2 repair, unpack) on the queue side, so a job is
//!   only [`DownloadState::Completed`] once it reaches history with a
//!   `SUCCESS`/`HEALTH` status; the final path is `DestDir`.
//! - **Remove** via `editqueue`/`HistoryFinalDelete` (Usenet does not seed →
//!   unconditional).
//!
//! Basic auth failure surfaces as HTTP 401, mapped to [`DownloadError::Auth`].

use cellarr_core::{DownloadState, GrabRequest};
use serde::Deserialize;
use serde_json::json;

use crate::error::DownloadError;
use crate::http::{HttpRequest, HttpTransport};
use crate::lifecycle::DownloadProgress;

/// Connection + auth settings for NZBGet.
#[derive(Debug, Clone, Deserialize)]
pub struct NzbgetSettings {
    /// Base URL, e.g. `http://localhost:6789` (no trailing slash).
    pub base_url: String,
    /// Control username (HTTP Basic).
    pub username: String,
    /// Control password (HTTP Basic).
    pub password: String,
}

/// An NZBGet download client.
pub struct NzbgetClient {
    name: String,
    settings: NzbgetSettings,
    category: String,
    transport: Box<dyn HttpTransport>,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Debug, Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}

/// One row from `listgroups`.
#[derive(Debug, Deserialize)]
struct Group {
    #[serde(rename = "NZBID")]
    nzb_id: i64,
    #[serde(rename = "Status", default)]
    status: String,
    #[serde(rename = "Category", default)]
    category: String,
    #[serde(rename = "FileSizeMB", default)]
    file_size_mb: i64,
    #[serde(rename = "RemainingSizeMB", default)]
    remaining_size_mb: i64,
}

/// One row from `history`.
#[derive(Debug, Deserialize)]
struct HistoryItem {
    #[serde(rename = "NZBID")]
    nzb_id: i64,
    /// e.g. `SUCCESS/ALL`, `SUCCESS/HEALTH`, `FAILURE/PAR`, `FAILURE/UNPACK`.
    #[serde(rename = "Status", default)]
    status: String,
    #[serde(rename = "Category", default)]
    category: String,
    #[serde(rename = "DestDir", default)]
    dest_dir: String,
}

impl NzbgetClient {
    /// Build a client over the production HTTP transport.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        settings: NzbgetSettings,
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
        settings: NzbgetSettings,
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

    /// The category cellarr files its downloads under.
    #[must_use]
    pub fn category(&self) -> &str {
        &self.category
    }

    fn base(&self) -> &str {
        self.settings.base_url.trim_end_matches('/')
    }

    /// Build the `Authorization: Basic …` header value.
    fn basic_auth(&self) -> String {
        let raw = format!("{}:{}", self.settings.username, self.settings.password);
        format!("Basic {}", base64_encode(raw.as_bytes()))
    }

    /// Make a JSON-RPC call with positional params and deserialize `result`.
    async fn call<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T, DownloadError> {
        let payload = json!({
            "method": method,
            "params": params,
            "id": 1,
            "jsonrpc": "2.0",
        });
        let req = HttpRequest::new("POST", format!("{}/jsonrpc", self.base()))
            .header("Content-Type", "application/json")
            .header("Authorization", self.basic_auth())
            .body(payload.to_string());
        let resp = self.transport.send(req).await?;
        if resp.status == 401 {
            return Err(DownloadError::Auth(
                "NZBGet rejected credentials (401)".into(),
            ));
        }
        if !resp.is_success() {
            return Err(DownloadError::Api(format!(
                "{method} returned status {}",
                resp.status
            )));
        }
        let parsed: RpcResponse<T> = serde_json::from_str(&resp.body)
            .map_err(|e| DownloadError::UnexpectedResponse(format!("{method}: {e}")))?;
        if let Some(err) = parsed.error {
            return Err(DownloadError::Api(format!(
                "{method} JSON-RPC error {}: {}",
                err.code, err.message
            )));
        }
        parsed
            .result
            .ok_or_else(|| DownloadError::UnexpectedResponse(format!("{method}: no result")))
    }

    /// Add an NZB by URL; returns the NZBGet id as a string.
    ///
    /// `append` positional params (in order): `NZBFilename`, `Content`,
    /// `Category`, `Priority`, `AddToTop`, `AddPaused`, `DupeKey`, `DupeScore`,
    /// `DupeMode`. We pass the URL as `Content` (NZBGet fetches URLs), an empty
    /// filename so NZBGet derives it, and cellarr's category.
    pub async fn add(&self, grab: &GrabRequest) -> Result<String, DownloadError> {
        let params = json!([
            "",                        // NZBFilename (derived from URL)
            grab.release.download_url, // Content: a URL NZBGet will fetch
            self.category,             // Category
            0,                         // Priority
            false,                     // AddToTop
            false,                     // AddPaused
            "",                        // DupeKey
            0,                         // DupeScore
            "SCORE",                   // DupeMode
        ]);
        let id: i64 = self.call("append", params).await?;
        if id <= 0 {
            return Err(DownloadError::Api(format!(
                "NZBGet rejected append (returned id {id})"
            )));
        }
        Ok(id.to_string())
    }

    /// Poll detailed progress for an NZBGet id.
    pub async fn progress(&self, download_id: &str) -> Result<DownloadProgress, DownloadError> {
        let id: i64 = download_id
            .parse()
            .map_err(|_| DownloadError::Config(format!("invalid NZBGet id {download_id:?}")))?;

        // Active jobs are in listgroups.
        let groups: Vec<Group> = self.call("listgroups", json!([0])).await?;
        if let Some(g) = groups.iter().find(|g| g.nzb_id == id) {
            return Ok(progress_from_group(g));
        }

        // Finished jobs are in history.
        let history: Vec<HistoryItem> = self.call("history", json!([false])).await?;
        let item = history
            .iter()
            .find(|h| h.nzb_id == id)
            .ok_or_else(|| DownloadError::NotFound(download_id.to_string()))?;
        Ok(progress_from_history(item))
    }

    /// Poll the coarse [`DownloadState`] for an NZBGet id.
    pub async fn status(&self, download_id: &str) -> Result<DownloadState, DownloadError> {
        Ok(self.progress(download_id).await?.state)
    }

    /// Remove a finished job from history (Usenet does not seed → unconditional).
    ///
    /// `editqueue` positional params: `Command`, `Offset`, `Text`, `IDs`.
    /// `HistoryFinalDelete` purges the job and (with NZBGet's own setting) its
    /// files; `HistoryDelete` keeps files. We choose per `delete_data`.
    pub async fn remove(&self, download_id: &str, delete_data: bool) -> Result<(), DownloadError> {
        let id: i64 = download_id
            .parse()
            .map_err(|_| DownloadError::Config(format!("invalid NZBGet id {download_id:?}")))?;
        let command = if delete_data {
            "HistoryFinalDelete"
        } else {
            "HistoryDelete"
        };
        let params = json!([command, 0, "", [id]]);
        let ok: bool = self.call("editqueue", params).await?;
        if !ok {
            return Err(DownloadError::Api(format!(
                "NZBGet editqueue {command} returned false"
            )));
        }
        Ok(())
    }
}

fn progress_from_group(g: &Group) -> DownloadProgress {
    let progress = if g.file_size_mb > 0 {
        let done = (g.file_size_mb - g.remaining_size_mb).max(0) as f64;
        (done / g.file_size_mb as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    // NZBGet queue Status values: QUEUED, PAUSED, DOWNLOADING, FETCHING,
    // PP_QUEUED, LOADING_PARS, VERIFYING_SOURCES, REPAIRING, UNPACKING,
    // POST_PROCESSING. None of these is importable yet — post-processing happens
    // before the job leaves the queue.
    let state = match g.status.as_str() {
        "QUEUED" | "PAUSED" => DownloadState::Queued,
        _ => DownloadState::Downloading,
    };
    DownloadProgress {
        state,
        progress,
        content_path: None,
        ratio: None,
        seeding_time_secs: None,
        peers: None,
        error_string: None,
        category: if g.category.is_empty() {
            None
        } else {
            Some(g.category.clone())
        },
    }
}

fn progress_from_history(h: &HistoryItem) -> DownloadProgress {
    let category = if h.category.is_empty() {
        None
    } else {
        Some(h.category.clone())
    };
    // History Status is `KIND/DETAIL`; SUCCESS/* and WARNING/HEALTH mean the
    // content is on disk and importable, FAILURE/* (incl. par/unpack failures)
    // is terminal.
    let kind = h.status.split('/').next().unwrap_or("");
    let state = match kind {
        "SUCCESS" | "WARNING" => DownloadState::Completed,
        "FAILURE" | "DELETED" => DownloadState::Failed,
        _ => DownloadState::Downloading,
    };
    DownloadProgress {
        state,
        progress: 1.0,
        content_path: if matches!(state, DownloadState::Completed) && !h.dest_dir.is_empty() {
            Some(h.dest_dir.clone())
        } else {
            None
        },
        ratio: None,
        seeding_time_secs: None,
        peers: None,
        error_string: matches!(state, DownloadState::Failed).then(|| h.status.clone()),
        category,
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
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(
            base64_encode(b"nzbget:tegbzn6789"),
            "bnpiZ2V0OnRlZ2J6bjY3ODk="
        );
    }
}
