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
use std::time::Duration;

use async_trait::async_trait;

use crate::error::DownloadError;

/// The default per-call HTTP timeout for the live transport.
///
/// Download-client calls are local/LAN and must never wedge the pipeline: a
/// single request that hangs (an unresponsive WebUI, a half-open connection)
/// is bounded here so every adapter call fails fast rather than blocking a job
/// indefinitely (see `docs/06-integrations.md` — status tracking must not be a
/// tight or unbounded loop).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

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
    /// A **binary** request body, if any. When set this takes precedence over
    /// [`body`](Self::body) on the wire (the production transport sends these bytes
    /// verbatim) — needed for the qBittorrent multipart `.torrent` upload, whose
    /// metainfo bytes are not valid UTF-8 and must not pass through a `String`.
    /// Contract tests still assert against the textual [`body`](Self::body), which
    /// the binary-body builder also sets to a lossy view for inspection.
    pub body_bytes: Option<Vec<u8>>,
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
            body_bytes: None,
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

    /// Attach a **binary** request body sent verbatim by the production transport.
    ///
    /// Also records a lossy textual [`body`](Self::body) so contract tests (which
    /// inspect the text body) can still assert on the multipart envelope's ASCII
    /// framing without the (binary) payload corrupting the `String`.
    #[must_use]
    pub fn body_bytes(mut self, bytes: Vec<u8>) -> Self {
        self.body = Some(String::from_utf8_lossy(&bytes).into_owned());
        self.body_bytes = Some(bytes);
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

/// One HTTP response carrying its **raw body bytes** rather than a `String`.
///
/// The text [`HttpResponse`] is enough for the JSON/form WebUI APIs, but
/// resolving a release's download URL to a submittable torrent source may fetch a
/// binary `.torrent` file (whose bytes must survive verbatim — they are not valid
/// UTF-8) and must inspect redirect status/`Location` without auto-following. This
/// raw shape carries exactly that: the status, the headers (for `Location`), and
/// the untouched body bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawHttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers as `name → value`, names lowercased.
    pub headers: BTreeMap<String, String>,
    /// Raw, undecoded response body bytes.
    pub body: Vec<u8>,
}

impl RawHttpResponse {
    /// Whether the status is in the 2xx success range.
    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Whether the status is a 3xx redirect.
    #[must_use]
    pub fn is_redirect(&self) -> bool {
        (300..400).contains(&self.status)
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

    /// Send one request and return its **raw** response, **without** following
    /// redirects.
    ///
    /// This is the seam the torrent-source resolver uses: it must see a 3xx
    /// `Location` (to detect an HTTP→magnet redirect and to follow HTTP→HTTP hops
    /// itself, bounded) and must receive a `.torrent` body as untouched bytes.
    /// A `reqwest` transport that auto-follows redirects would hide both, so this
    /// is a distinct method rather than a flavour of [`send`](Self::send).
    ///
    /// The default implementation delegates to [`send`](Self::send) and treats the
    /// (lossy-decoded) text body as bytes — adequate for the replay transport in
    /// tests, which carries text/base64 fixtures. The production transport
    /// overrides it to fetch true bytes with redirects disabled.
    async fn send_raw(&self, req: HttpRequest) -> Result<RawHttpResponse, DownloadError> {
        let resp = self.send(req).await?;
        Ok(RawHttpResponse {
            status: resp.status,
            headers: resp.headers,
            body: resp.body.into_bytes(),
        })
    }
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
    ///
    /// A [`DEFAULT_TIMEOUT`] is applied to every request so a single hung call
    /// can never wedge a tracking job; build with [`with_timeout`](Self::with_timeout)
    /// to override it.
    #[must_use]
    pub fn new() -> Self {
        Self::with_timeout(DEFAULT_TIMEOUT)
    }

    /// Build a transport whose every request is bounded by `timeout`.
    ///
    /// Redirects are **not** auto-followed: the WebUI APIs never redirect, and the
    /// torrent-source resolver must inspect a 3xx `Location` itself (an HTTP→magnet
    /// redirect, or a bounded HTTP→HTTP hop) rather than have `reqwest` swallow it.
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            // A client with an explicit timeout is always buildable; fall back
            // to the default client rather than panic in the unreachable case.
            .unwrap_or_default();
        Self { client }
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
        if let Some(bytes) = req.body_bytes {
            builder = builder.body(bytes);
        } else if let Some(body) = req.body {
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

    async fn send_raw(&self, req: HttpRequest) -> Result<RawHttpResponse, DownloadError> {
        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|e| DownloadError::Transport(e.to_string()))?;
        let mut builder = self.client.request(method, &req.url);
        for (name, value) in &req.headers {
            builder = builder.header(name, value);
        }
        if let Some(bytes) = req.body_bytes {
            builder = builder.body(bytes);
        } else if let Some(body) = req.body {
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
            .bytes()
            .await
            .map_err(|e| DownloadError::Transport(e.to_string()))?
            .to_vec();
        Ok(RawHttpResponse {
            status,
            headers,
            body,
        })
    }
}
