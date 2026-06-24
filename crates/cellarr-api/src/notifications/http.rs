//! The minimal HTTP seam the HTTP-backed notification providers post through.
//!
//! Discord, Telegram, the generic Webhook, and the media-server rescan providers
//! (Plex/Jellyfin/Emby) all talk HTTP. They post through an [`HttpClient`] rather
//! than calling `reqwest` directly so the provider logic stays pure and the
//! record/replay tests can assert the exact request (method, URL, headers, body)
//! each provider builds — without a live service or any network. This mirrors the
//! `Fetcher` seam the indexer adapters use.

use std::collections::BTreeMap;

use async_trait::async_trait;

/// One outbound HTTP request a provider builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// The HTTP method (`POST`/`GET`).
    pub method: HttpMethod,
    /// The absolute request URL.
    pub url: String,
    /// Extra request headers (content-type, auth). Ordered for stable assertions.
    pub headers: BTreeMap<String, String>,
    /// The request body (a JSON or form payload), empty for a bodyless GET.
    pub body: Vec<u8>,
}

/// The HTTP methods the providers use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// `GET` — the media-server liveness pings and library scans.
    Get,
    /// `POST` — the message/embed deliveries.
    Post,
}

impl HttpRequest {
    /// A JSON `POST` with the `application/json` content type set.
    #[must_use]
    pub fn json_post(url: impl Into<String>, body: &serde_json::Value) -> Self {
        let mut headers = BTreeMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        Self {
            method: HttpMethod::Post,
            url: url.into(),
            headers,
            body: serde_json::to_vec(body).unwrap_or_default(),
        }
    }

    /// A `GET` with no body (a liveness ping or a library-scan trigger).
    #[must_use]
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            url: url.into(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }

    /// Set a header (builder form).
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }
}

/// The HTTP response a provider inspects.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// The status code.
    pub status: u16,
    /// The response body (read for diagnostics; small for these APIs).
    pub body: String,
}

impl HttpResponse {
    /// Whether the status is a 2xx success.
    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Something that can execute an [`HttpRequest`]. The provider seam: production
/// uses the `reqwest`-backed [`ReqwestHttpClient`]; tests use a recording mock.
#[async_trait]
pub trait HttpClient: Send + Sync {
    /// Execute `request`, returning the response or a transport-level error
    /// string (a connection failure, timeout, or malformed URL — never a non-2xx
    /// status, which the caller reads off [`HttpResponse::status`]).
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, String>;
}

/// A `reqwest`-backed [`HttpClient`] with a bounded per-request timeout so a hung
/// receiver never stalls the dispatcher.
#[derive(Clone)]
pub struct ReqwestHttpClient {
    client: reqwest::Client,
}

impl ReqwestHttpClient {
    /// Build a client with the default bounded timeout.
    #[must_use]
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl Default for ReqwestHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, String> {
        let mut builder = match request.method {
            HttpMethod::Get => self.client.get(&request.url),
            HttpMethod::Post => self.client.post(&request.url),
        };
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body);
        }
        let resp = builder
            .send()
            .await
            .map_err(|e| format!("HTTP request to {} failed: {e}", request.url))?;
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Ok(HttpResponse { status, body })
    }
}
