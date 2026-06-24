//! SABnzbd adapter.
//!
//! SABnzbd exposes one HTTP API selected by a `mode=` query param, authenticated
//! with `apikey=`, returning JSON when `output=json` is set (see
//! `docs/06-integrations.md`). The adapter:
//!
//! - **Adds** via `mode=addurl` with `cat=<cellarr category>`, returning the
//!   `nzo_id` SABnzbd assigns.
//! - **Polls** completion across two surfaces, because a job moves between them:
//!   while downloading/repairing/unpacking it is in `mode=queue`; once finished
//!   it leaves the queue and appears in `mode=history`. **Completion is only
//!   reported after history shows the job `Completed`** — i.e. after repair and
//!   unpack — with the final `storage` path for Import.
//! - **Removes** via `mode=history` `name=delete` (Usenet does not seed, so
//!   removal is unconditional).
//!
//! An auth failure surfaces as SABnzbd's `{"status": false, "error": "API Key
//! Incorrect"}`, which we map to [`DownloadError::Auth`].

use cellarr_core::{DownloadState, GrabRequest};
use serde::Deserialize;

use crate::error::DownloadError;
use crate::http::{HttpRequest, HttpTransport};
use crate::lifecycle::DownloadProgress;

/// Connection + auth settings for SABnzbd.
#[derive(Debug, Clone, Deserialize)]
pub struct SabnzbdSettings {
    /// Base URL, e.g. `http://localhost:8080` (no trailing slash).
    pub base_url: String,
    /// API key.
    pub api_key: String,
}

/// A SABnzbd download client.
pub struct SabnzbdClient {
    name: String,
    settings: SabnzbdSettings,
    category: String,
    transport: Box<dyn HttpTransport>,
}

#[derive(Debug, Deserialize)]
struct AddUrlResponse {
    status: bool,
    #[serde(default)]
    nzo_ids: Vec<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QueueEnvelope {
    queue: QueueBody,
}

#[derive(Debug, Deserialize)]
struct QueueBody {
    #[serde(default)]
    slots: Vec<QueueSlot>,
}

#[derive(Debug, Deserialize)]
struct QueueSlot {
    nzo_id: String,
    /// e.g. `Downloading`, `Queued`, `Paused`, `Checking`, `Repairing`,
    /// `Extracting`.
    status: String,
    #[serde(default)]
    cat: String,
    /// Percent complete as a string, e.g. `"42"`.
    #[serde(default)]
    percentage: String,
}

#[derive(Debug, Deserialize)]
struct HistoryEnvelope {
    history: HistoryBody,
}

#[derive(Debug, Deserialize)]
struct HistoryBody {
    #[serde(default)]
    slots: Vec<HistorySlot>,
}

#[derive(Debug, Deserialize)]
struct HistorySlot {
    nzo_id: String,
    /// e.g. `Completed`, `Failed`, `Extracting`, `Verifying`.
    status: String,
    #[serde(default)]
    category: String,
    /// Final on-disk path of the unpacked content.
    #[serde(default)]
    storage: String,
    /// SABnzbd's failure detail for a `Failed` slot, when present.
    #[serde(default)]
    fail_message: String,
}

/// SABnzbd's `{"status": false, "error": "..."}` error envelope.
#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    status: bool,
    error: String,
}

impl SabnzbdClient {
    /// Build a client over the production HTTP transport.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        settings: SabnzbdSettings,
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
        settings: SabnzbdSettings,
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

    /// Build an `…/api?mode=…&output=json&apikey=…` URL with extra params.
    fn api_url(&self, mode: &str, extra: &[(&str, &str)]) -> String {
        let mut url = format!(
            "{}/api?mode={}&output=json&apikey={}",
            self.base(),
            mode,
            urlencode(&self.settings.api_key)
        );
        for (k, v) in extra {
            url.push('&');
            url.push_str(k);
            url.push('=');
            url.push_str(&urlencode(v));
        }
        url
    }

    /// Detect SABnzbd's auth-failure envelope in a response body.
    fn check_auth_error(body: &str) -> Result<(), DownloadError> {
        if let Ok(err) = serde_json::from_str::<ErrorEnvelope>(body) {
            if !err.status {
                let lower = err.error.to_ascii_lowercase();
                if lower.contains("api key") || lower.contains("apikey") {
                    return Err(DownloadError::Auth(err.error));
                }
                return Err(DownloadError::Api(err.error));
            }
        }
        Ok(())
    }

    /// Add an NZB by URL; returns the `nzo_id`.
    pub async fn add(&self, grab: &GrabRequest) -> Result<String, DownloadError> {
        let url = self.api_url(
            "addurl",
            &[
                ("name", &grab.release.download_url),
                ("cat", &self.category),
            ],
        );
        let resp = self.transport.send(HttpRequest::new("GET", url)).await?;
        if !resp.is_success() {
            return Err(DownloadError::Api(format!(
                "addurl returned status {}",
                resp.status
            )));
        }
        Self::check_auth_error(&resp.body)?;
        let parsed: AddUrlResponse = serde_json::from_str(&resp.body)
            .map_err(|e| DownloadError::UnexpectedResponse(format!("addurl: {e}")))?;
        if !parsed.status {
            return Err(DownloadError::Api(
                parsed.error.unwrap_or_else(|| "addurl failed".into()),
            ));
        }
        parsed
            .nzo_ids
            .into_iter()
            .next()
            .ok_or_else(|| DownloadError::UnexpectedResponse("addurl returned no nzo_id".into()))
    }

    /// Poll detailed progress for an `nzo_id` across queue then history.
    pub async fn progress(&self, nzo_id: &str) -> Result<DownloadProgress, DownloadError> {
        // While active, the job is in the queue.
        let queue_url = self.api_url("queue", &[]);
        let resp = self
            .transport
            .send(HttpRequest::new("GET", queue_url))
            .await?;
        Self::check_auth_error(&resp.body)?;
        if resp.is_success() {
            if let Ok(env) = serde_json::from_str::<QueueEnvelope>(&resp.body) {
                if let Some(slot) = env.queue.slots.iter().find(|s| s.nzo_id == nzo_id) {
                    return Ok(progress_from_queue(slot));
                }
            }
        }

        // Otherwise it has finished (or failed) and moved to history.
        let history_url = self.api_url("history", &[]);
        let resp = self
            .transport
            .send(HttpRequest::new("GET", history_url))
            .await?;
        Self::check_auth_error(&resp.body)?;
        let env: HistoryEnvelope = serde_json::from_str(&resp.body)
            .map_err(|e| DownloadError::UnexpectedResponse(format!("history: {e}")))?;
        let slot = env
            .history
            .slots
            .iter()
            .find(|s| s.nzo_id == nzo_id)
            .ok_or_else(|| DownloadError::NotFound(nzo_id.to_string()))?;
        progress_from_history(slot)
    }

    /// Poll the coarse [`DownloadState`] for an `nzo_id`.
    pub async fn status(&self, nzo_id: &str) -> Result<DownloadState, DownloadError> {
        Ok(self.progress(nzo_id).await?.state)
    }

    /// Remove a job. Usenet does not seed, so removal is unconditional; the job
    /// is deleted from history (and its files when `delete_data`).
    pub async fn remove(&self, nzo_id: &str, delete_data: bool) -> Result<(), DownloadError> {
        let del_files = if delete_data { "1" } else { "0" };
        let url = self.api_url(
            "history",
            &[
                ("name", "delete"),
                ("value", nzo_id),
                ("del_files", del_files),
            ],
        );
        let resp = self.transport.send(HttpRequest::new("GET", url)).await?;
        Self::check_auth_error(&resp.body)?;
        if !resp.is_success() {
            return Err(DownloadError::Api(format!(
                "history delete returned status {}",
                resp.status
            )));
        }
        Ok(())
    }
}

fn progress_from_queue(slot: &QueueSlot) -> DownloadProgress {
    let progress = slot.percentage.parse::<f64>().unwrap_or(0.0) / 100.0;
    let state = match slot.status.as_str() {
        "Queued" | "Paused" => DownloadState::Queued,
        // Downloading, Checking, Repairing, Extracting, Verifying, Fetching,
        // Grabbing — all still in flight, not yet importable.
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
        category: if slot.cat.is_empty() {
            None
        } else {
            Some(slot.cat.clone())
        },
    }
}

fn progress_from_history(slot: &HistorySlot) -> Result<DownloadProgress, DownloadError> {
    let category = if slot.category.is_empty() {
        None
    } else {
        Some(slot.category.clone())
    };
    match slot.status.as_str() {
        "Completed" => Ok(DownloadProgress {
            state: DownloadState::Completed,
            progress: 1.0,
            content_path: if slot.storage.is_empty() {
                None
            } else {
                Some(slot.storage.clone())
            },
            ratio: None,
            seeding_time_secs: None,
            peers: None,
            error_string: None,
            category,
        }),
        "Failed" => Ok(DownloadProgress {
            state: DownloadState::Failed,
            progress: 1.0,
            content_path: None,
            ratio: None,
            seeding_time_secs: None,
            peers: None,
            error_string: (!slot.fail_message.is_empty()).then(|| slot.fail_message.clone()),
            category,
        }),
        // Still post-processing in history (Extracting/Verifying/Repairing):
        // not yet importable — repair/unpack must finish first.
        _ => Ok(DownloadProgress {
            state: DownloadState::Downloading,
            progress: 1.0,
            content_path: None,
            ratio: None,
            seeding_time_secs: None,
            peers: None,
            error_string: None,
            category,
        }),
    }
}

/// Minimal query-value urlencoder (see qBittorrent adapter for rationale).
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
