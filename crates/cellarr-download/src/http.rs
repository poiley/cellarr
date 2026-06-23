//! The HTTP seam every adapter talks through.
//!
//! Download clients are tested with **record/replay** fixtures and never against
//! live services in CI (see `docs/06-integrations.md`). The way we make that
//! possible without a recording proxy is to route every adapter's HTTP I/O
//! through one small trait, [`HttpTransport`]. Production wires in
//! [`ReqwestTransport`]; tests wire in a replay transport that returns recorded
//! responses and asserts the requests the adapter made.
//!
//! Keeping the seam this narrow (a single request→response method over plain
//! data) is deliberate: it captures exactly what a contract test needs to pin —
//! method, URL, headers (so the qBittorrent `Referer`/`Origin`/`SID` quirks are
//! observable), and body — and nothing else.

use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::error::DownloadError;

/// One HTTP request as the adapter wants it sent.
///
/// Headers are an ordered map so a fixture can assert the exact set an adapter
/// attaches (the qBittorrent `Referer`/`Origin`/cookie handling is the reason
/// this is captured rather than hidden inside `reqwest`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// HTTP method, uppercase (`GET`, `POST`).
    pub method: String,
    /// Fully-qualified request URL including any query string.
    pub url: String,
    /// Request headers as `name → value`. Names are stored lowercase so
    /// comparison is case-insensitive, matching HTTP semantics.
    pub headers: BTreeMap<String, String>,
    /// Request body, if any (form-encoded or JSON depending on the adapter).
    pub body: Option<String>,
}

impl HttpRequest {
    /// Start a request builder for `method` and `url`.
    #[must_use]
    pub fn new(method: &str, url: impl Into<String>) -> Self {
        Self {
            method: method.to_ascii_uppercase(),
            url: url.into(),
            headers: BTreeMap::new(),
            body: None,
        }
    }

    /// Attach a header, lowercasing the name for case-insensitive comparison.
    #[must_use]
    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.insert(name.to_ascii_lowercase(), value.into());
        self
    }

    /// Attach a request body.
    #[must_use]
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }
}

/// One HTTP response as the transport returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers as `name → value`, names lowercased.
    pub headers: BTreeMap<String, String>,
    /// Response body.
    pub body: String,
}

impl HttpResponse {
    /// Whether the status is in the 2xx success range.
    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Read a response header case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

/// The HTTP seam adapters depend on instead of `reqwest` directly.
///
/// Object-safe so a client can hold `Box<dyn HttpTransport>` and be configured
/// with either the real transport or a replay transport in tests.
#[async_trait]
pub trait HttpTransport: Send + Sync {
    /// Send one request and return the response, or a transport error.
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, DownloadError>;
}

/// The production transport: a thin wrapper over `reqwest`.
#[derive(Debug)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    /// Build a transport over a fresh `reqwest` client.
    ///
    /// We do **not** enable `reqwest`'s cookie store: the qBittorrent adapter
    /// manages its `SID` cookie explicitly so the contract tests can observe it
    /// on the wire, which is exactly the behavior that broke in qBittorrent 5.x.
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// Build a transport over a caller-supplied `reqwest` client.
    #[must_use]
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, DownloadError> {
        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|e| DownloadError::Transport(e.to_string()))?;
        let mut builder = self.client.request(method, &req.url);
        for (name, value) in &req.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = req.body {
            builder = builder.body(body);
        }
        let resp = builder
            .send()
            .await
            .map_err(|e| DownloadError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let mut headers = BTreeMap::new();
        for (name, value) in resp.headers() {
            if let Ok(v) = value.to_str() {
                headers.insert(name.as_str().to_ascii_lowercase(), v.to_string());
            }
        }
        let body = resp
            .text()
            .await
            .map_err(|e| DownloadError::Transport(e.to_string()))?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}
