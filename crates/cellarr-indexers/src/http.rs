//! The minimal HTTP seam used by indexer adapters.
//!
//! Adapters fetch documents through a [`Fetcher`] rather than calling `reqwest`
//! directly. That keeps the protocol logic pure and lets the record/replay tests
//! feed recorded `t=caps`/search responses without any network — live indexers
//! are never a test dependency (`docs/06-integrations.md`).

use async_trait::async_trait;
use tokio::sync::OnceCell;

use crate::error::{IndexerError, Result};

/// Something that can fetch a URL and return its body as text.
#[async_trait]
pub trait Fetcher: Send + Sync {
    /// GET `url` and return the response body as a string.
    async fn get(&self, url: &str) -> Result<String>;

    /// POST `body` (with `content_type`) to `url` and return the response body.
    ///
    /// Defaults to unsupported: only a fetcher that needs form-POST search (some
    /// Cardigann trackers) implements it. Record/replay test fetchers override it
    /// when a test exercises a POST-method definition.
    async fn post(&self, url: &str, _body: &str, _content_type: &str) -> Result<String> {
        Err(IndexerError::Unsupported(format!(
            "POST to {url} (this fetcher is GET-only)"
        )))
    }

    /// GET `url` and return the **raw, undecoded** response bytes.
    ///
    /// Needed by the Cardigann engine to honor a definition's declared `encoding`
    /// (e.g. `windows-1251`) when the server sends no/incorrect charset header. The
    /// default decodes nothing — it returns [`Fetcher::get`]'s UTF-8 bytes, which is
    /// correct for the UTF-8 case and for the string-replay test fetchers.
    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        self.get(url).await.map(String::into_bytes)
    }

    /// POST and return the **raw, undecoded** response bytes (see [`get_bytes`]).
    ///
    /// [`get_bytes`]: Fetcher::get_bytes
    async fn post_bytes(&self, url: &str, body: &str, content_type: &str) -> Result<Vec<u8>> {
        self.post(url, body, content_type)
            .await
            .map(String::into_bytes)
    }
}

/// A `reqwest`-backed fetcher for production use.
pub struct ReqwestFetcher {
    client: reqwest::Client,
}

impl ReqwestFetcher {
    /// Build a fetcher from an existing client (so callers control timeouts,
    /// proxies, and connection pooling centrally).
    #[must_use]
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for ReqwestFetcher {
    fn default() -> Self {
        Self::new(reqwest::Client::new())
    }
}

impl ReqwestFetcher {
    /// Read a response body, turning a non-success status into [`IndexerError::Status`]
    /// (a 403/429 here is the canonical "banned / rate-limited" signal).
    async fn read_body(resp: reqwest::Response) -> Result<String> {
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(200).collect();
            return Err(IndexerError::Status {
                status: status.as_u16(),
                body_snippet: (!snippet.is_empty()).then_some(snippet),
            });
        }
        Ok(resp.text().await?)
    }

    /// Read a response as raw bytes, with the same non-success → [`IndexerError::Status`]
    /// handling as [`read_body`](Self::read_body).
    async fn read_raw(resp: reqwest::Response) -> Result<Vec<u8>> {
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(200).collect();
            return Err(IndexerError::Status {
                status: status.as_u16(),
                body_snippet: (!snippet.is_empty()).then_some(snippet),
            });
        }
        Ok(resp.bytes().await?.to_vec())
    }
}

/// A [`Fetcher`] that proxies through a **FlareSolverr** instance.
///
/// Some trackers sit behind an interstitial bot check that a plain HTTP client
/// cannot clear — the request never reaches the site, so no amount of definition
/// tuning helps. FlareSolverr drives a real browser, solves the challenge, and
/// returns the resulting document; keeping a named session across calls preserves
/// the clearance cookies so only the first request pays the solve cost.
///
/// This is **opt-in per indexer**. It introduces an external service, which the
/// single-binary/zero-required-services default must not depend on, so it is only
/// constructed when an operator configures an endpoint for a specific indexer.
pub struct FlareSolverrFetcher {
    client: reqwest::Client,
    /// Base URL of the FlareSolverr instance (its `/v1` endpoint is appended).
    endpoint: String,
    /// Session name, so clearance cookies persist across requests.
    session: String,
    /// How long FlareSolverr may spend solving, in milliseconds.
    max_timeout_ms: u64,
    /// Ensures the session is created once, on first use.
    session_ready: OnceCell<()>,
}

impl FlareSolverrFetcher {
    /// Default solve budget. A challenge typically clears in 15-20s; below that a
    /// first request against a protected host fails spuriously.
    const DEFAULT_MAX_TIMEOUT_MS: u64 = 90_000;

    /// Build a fetcher pointed at a FlareSolverr instance (e.g.
    /// `http://flaresolverr:8191/`), tagging its session with `session`.
    #[must_use]
    pub fn new(
        client: reqwest::Client,
        endpoint: impl Into<String>,
        session: impl Into<String>,
    ) -> Self {
        Self {
            client,
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            session: session.into(),
            max_timeout_ms: Self::DEFAULT_MAX_TIMEOUT_MS,
            session_ready: OnceCell::new(),
        }
    }

    /// Build against `endpoint` with a client of this crate's choosing, so a caller
    /// that only has configuration (an endpoint string) needs no `reqwest` of its own.
    ///
    /// The client's timeout must exceed the solve budget — a challenge routinely
    /// takes 15-20s and the browser holds the request open for the whole of it, so a
    /// default-timeout client would abort every protected fetch.
    #[must_use]
    pub fn with_endpoint(endpoint: impl Into<String>, session: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(
                Self::DEFAULT_MAX_TIMEOUT_MS + 30_000,
            ))
            .build()
            .unwrap_or_default();
        Self::new(client, endpoint, session)
    }

    /// Override the solve budget in milliseconds.
    #[must_use]
    pub fn with_max_timeout_ms(mut self, max_timeout_ms: u64) -> Self {
        self.max_timeout_ms = max_timeout_ms;
        self
    }

    /// POST one command to FlareSolverr and return the decoded envelope.
    async fn command(&self, payload: serde_json::Value) -> Result<serde_json::Value> {
        let resp = self
            .client
            .post(format!("{}/v1", self.endpoint))
            .json(&payload)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            let snippet: String = body.chars().take(200).collect();
            return Err(IndexerError::Status {
                status: status.as_u16(),
                body_snippet: (!snippet.is_empty()).then_some(snippet),
            });
        }
        serde_json::from_str(&body)
            .map_err(|e| IndexerError::Parse(format!("flaresolverr envelope: {e}")))
    }

    /// Create the named session once; a session that already exists is not an error.
    async fn ensure_session(&self) {
        self.session_ready
            .get_or_init(|| async {
                let _ = self
                    .command(serde_json::json!({
                        "cmd": "sessions.create",
                        "session": self.session,
                    }))
                    .await;
            })
            .await;
    }

    /// Run a `request.get`/`request.post` and return the page body.
    async fn request(&self, payload: serde_json::Value) -> Result<String> {
        self.ensure_session().await;
        let envelope = self.command(payload).await?;

        if envelope.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
            let message = envelope
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("no message");
            return Err(IndexerError::Parse(format!("flaresolverr: {message}")));
        }
        // The upstream status lives on the solution; FlareSolverr itself answers 200
        // even when the site returned an error page, so a non-success here must be
        // surfaced as the tracker being unavailable rather than as empty results.
        let solution = envelope
            .get("solution")
            .ok_or_else(|| IndexerError::Parse("flaresolverr reply had no solution".to_string()))?;
        let upstream = solution
            .get("status")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let body = solution
            .get("response")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !(200..300).contains(&upstream) {
            let snippet: String = body.chars().take(200).collect();
            return Err(IndexerError::Status {
                status: u16::try_from(upstream).unwrap_or(0),
                body_snippet: (!snippet.is_empty()).then_some(snippet),
            });
        }
        Ok(unwrap_pre(&body))
    }
}

/// Unwrap a non-HTML body from the `<pre>` block the headless browser renders it in.
///
/// A JSON endpoint fetched through FlareSolverr comes back as a rendered document,
/// not raw bytes, so callers expecting JSON would otherwise get markup. A body with
/// no single `<pre>` is returned unchanged.
fn unwrap_pre(body: &str) -> String {
    let Some(start) = body.find("<pre") else {
        return body.to_string();
    };
    let Some(open_end) = body[start..].find('>').map(|i| start + i + 1) else {
        return body.to_string();
    };
    let Some(close) = body[open_end..].find("</pre>").map(|i| open_end + i) else {
        return body.to_string();
    };
    let inner = &body[open_end..close];
    inner
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[async_trait]
impl Fetcher for FlareSolverrFetcher {
    async fn get(&self, url: &str) -> Result<String> {
        self.request(serde_json::json!({
            "cmd": "request.get",
            "url": url,
            "session": self.session,
            "maxTimeout": self.max_timeout_ms,
        }))
        .await
    }

    async fn post(&self, url: &str, body: &str, _content_type: &str) -> Result<String> {
        // FlareSolverr only submits form-encoded bodies; it derives the content type
        // itself, so the caller's is not forwarded.
        self.request(serde_json::json!({
            "cmd": "request.post",
            "url": url,
            "postData": body,
            "session": self.session,
            "maxTimeout": self.max_timeout_ms,
        }))
        .await
    }
}

#[async_trait]
impl Fetcher for ReqwestFetcher {
    async fn get(&self, url: &str) -> Result<String> {
        let resp = self.client.get(url).send().await?;
        Self::read_body(resp).await
    }

    async fn post(&self, url: &str, body: &str, content_type: &str) -> Result<String> {
        let resp = self
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(body.to_string())
            .send()
            .await?;
        Self::read_body(resp).await
    }

    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self.client.get(url).send().await?;
        Self::read_raw(resp).await
    }

    async fn post_bytes(&self, url: &str, body: &str, content_type: &str) -> Result<Vec<u8>> {
        let resp = self
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(body.to_string())
            .send()
            .await?;
        Self::read_raw(resp).await
    }
}
