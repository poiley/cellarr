//! The HTTP fetcher seam.
//!
//! Source adapters never call `reqwest` directly. They go through the
//! [`Fetcher`] trait, which has exactly two implementations:
//!
//! - [`ReqwestFetcher`] — the real, live transport (used by the daemon).
//! - [`RecordedFetcher`] — a map of `(method, url) -> body`, used by the
//!   record/replay tests so **no live source touches the CI path** (a hard
//!   requirement in `docs/07-metadata-service.md`).
//!
//! Keeping the seam this thin means the adapters' normalization logic — the part
//! worth testing — runs identically against recorded and live bytes.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::error::MetaError;

/// The outcome of one HTTP request: a status and a body. Adapters branch on the
/// status (auth/rate-limit/not-found) before decoding the body.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// The HTTP status code.
    pub status: u16,
    /// The response body bytes.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Whether the status is in the 2xx success range.
    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The transport seam every source adapter depends on.
#[async_trait]
pub trait Fetcher: Send + Sync {
    /// Issue a GET to `url` with the given headers (as `(name, value)` pairs).
    async fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, MetaError>;
}

/// The live transport, backed by `reqwest`.
pub struct ReqwestFetcher {
    client: reqwest::Client,
    /// Tag attached to errors so callers know which source failed.
    source: &'static str,
}

impl ReqwestFetcher {
    /// Build a live fetcher tagged with the owning source's name.
    ///
    /// The descriptive `User-Agent` is mandatory for some sources (MusicBrainz)
    /// and good manners for the rest; it identifies cellarr so source operators
    /// can reach us, as `docs/07-metadata-service.md` requires.
    #[must_use]
    pub fn new(source: &'static str) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("cellarr-meta/", env!("CARGO_PKG_VERSION")))
            .build()
            // A default client always builds; the only failure modes are TLS
            // backend init which rustls does not hit here. Fall back rather than
            // panic on a user-reachable path.
            .unwrap_or_default();
        Self { client, source }
    }
}

#[async_trait]
impl Fetcher for ReqwestFetcher {
    async fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, MetaError> {
        let mut req = self.client.get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = req.send().await.map_err(|e| MetaError::Transport {
            src: self.source,
            detail: e.to_string(),
        })?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| MetaError::Transport {
                src: self.source,
                detail: e.to_string(),
            })?
            .to_vec();
        Ok(HttpResponse { status, body })
    }
}

/// A replay transport: serves recorded bodies keyed by URL.
///
/// Lookups match on the URL *prefix* (path + query before any trailing token),
/// so tests can register a stable key without pinning a volatile API-key query
/// parameter. An unregistered URL yields a 404 so adapters exercise their
/// not-found path too.
#[derive(Default)]
pub struct RecordedFetcher {
    routes: HashMap<String, HttpResponse>,
}

impl RecordedFetcher {
    /// An empty recorder (every request 404s — models a source with no data).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a recorded 200 response for any URL that starts with `prefix`.
    #[must_use]
    pub fn with_body(mut self, prefix: &str, body: impl Into<Vec<u8>>) -> Self {
        self.routes.insert(
            prefix.to_string(),
            HttpResponse {
                status: 200,
                body: body.into(),
            },
        );
        self
    }

    /// Register a recorded response with an explicit status (e.g. 429/401) for
    /// any URL starting with `prefix`.
    #[must_use]
    pub fn with_response(mut self, prefix: &str, status: u16, body: impl Into<Vec<u8>>) -> Self {
        self.routes.insert(
            prefix.to_string(),
            HttpResponse {
                status,
                body: body.into(),
            },
        );
        self
    }
}

#[async_trait]
impl Fetcher for RecordedFetcher {
    async fn get(&self, url: &str, _headers: &[(&str, &str)]) -> Result<HttpResponse, MetaError> {
        // Longest matching prefix wins so a more specific route (e.g. an
        // episodes endpoint) is preferred over a broader series one.
        let best = self
            .routes
            .iter()
            .filter(|(prefix, _)| url.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len());
        match best {
            Some((_, resp)) => Ok(resp.clone()),
            None => Ok(HttpResponse {
                status: 404,
                body: Vec::new(),
            }),
        }
    }
}
