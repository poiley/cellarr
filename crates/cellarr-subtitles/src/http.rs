//! The HTTP fetcher seam (mirrors `cellarr-meta`'s).
//!
//! Providers never call `reqwest` directly — they go through [`Fetcher`], which
//! has two implementations: [`ReqwestFetcher`] (the live daemon transport) and
//! [`RecordedFetcher`] (a `url-prefix -> response` map for the record/replay
//! tests, so no live provider touches CI). Keeping the seam thin means a
//! provider's normalization logic runs identically against recorded and live
//! bytes.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::error::SubtitleError;

/// The outcome of one HTTP request: a status and a body.
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

/// The transport seam every provider depends on.
#[async_trait]
pub trait Fetcher: Send + Sync {
    /// Issue a GET to `url` with the given headers (as `(name, value)` pairs).
    async fn get(&self, url: &str, headers: &[(&str, &str)])
        -> Result<HttpResponse, SubtitleError>;

    /// Issue a POST to `url` with a JSON body and the given headers.
    async fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, SubtitleError>;
}

/// The live transport, backed by `reqwest`.
pub struct ReqwestFetcher {
    client: reqwest::Client,
    source: &'static str,
}

impl ReqwestFetcher {
    /// Build a live fetcher tagged with the owning provider's name. The
    /// descriptive `User-Agent` identifies cellarr — OpenSubtitles requires one.
    #[must_use]
    pub fn new(source: &'static str) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("cellarr-subtitles/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self { client, source }
    }
}

#[async_trait]
impl Fetcher for ReqwestFetcher {
    async fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, SubtitleError> {
        let mut req = self.client.get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = req.send().await.map_err(|e| SubtitleError::Transport {
            src: self.source,
            detail: e.to_string(),
        })?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| SubtitleError::Transport {
                src: self.source,
                detail: e.to_string(),
            })?
            .to_vec();
        Ok(HttpResponse { status, body })
    }

    async fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, SubtitleError> {
        let mut req = self.client.post(url).json(body);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = req.send().await.map_err(|e| SubtitleError::Transport {
            src: self.source,
            detail: e.to_string(),
        })?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| SubtitleError::Transport {
                src: self.source,
                detail: e.to_string(),
            })?
            .to_vec();
        Ok(HttpResponse { status, body })
    }
}

/// A replay transport: serves recorded bodies keyed by URL prefix (longest match
/// wins). An unregistered URL yields a 404 so a provider exercises its not-found
/// path too.
#[derive(Default)]
pub struct RecordedFetcher {
    routes: HashMap<String, HttpResponse>,
}

impl RecordedFetcher {
    /// An empty recorder (every request 404s).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a recorded 200 response for any URL starting with `prefix`.
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

    /// Register a recorded response with an explicit status for `prefix`.
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

    fn best(&self, url: &str) -> Option<HttpResponse> {
        self.routes
            .iter()
            .filter(|(prefix, _)| url.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, resp)| resp.clone())
    }
}

#[async_trait]
impl Fetcher for RecordedFetcher {
    async fn get(
        &self,
        url: &str,
        _headers: &[(&str, &str)],
    ) -> Result<HttpResponse, SubtitleError> {
        Ok(self.best(url).unwrap_or(HttpResponse {
            status: 404,
            body: Vec::new(),
        }))
    }

    async fn post_json(
        &self,
        url: &str,
        _body: &serde_json::Value,
        _headers: &[(&str, &str)],
    ) -> Result<HttpResponse, SubtitleError> {
        Ok(self.best(url).unwrap_or(HttpResponse {
            status: 404,
            body: Vec::new(),
        }))
    }
}
