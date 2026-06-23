//! The minimal HTTP seam used by indexer adapters.
//!
//! Adapters fetch documents through a [`Fetcher`] rather than calling `reqwest`
//! directly. That keeps the protocol logic pure and lets the record/replay tests
//! feed recorded `t=caps`/search responses without any network — live indexers
//! are never a test dependency (`docs/06-integrations.md`).

use async_trait::async_trait;

use crate::error::{IndexerError, Result};

/// Something that can fetch a URL and return its body as text.
#[async_trait]
pub trait Fetcher: Send + Sync {
    /// GET `url` and return the response body as a string.
    async fn get(&self, url: &str) -> Result<String>;
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

#[async_trait]
impl Fetcher for ReqwestFetcher {
    async fn get(&self, url: &str) -> Result<String> {
        let resp = self.client.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            // Keep a short body prefix for diagnostics; a 403/429 here is the
            // canonical "banned / rate-limited" signal the caller must act on.
            let body = resp.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(200).collect();
            return Err(IndexerError::Status {
                status: status.as_u16(),
                body_snippet: (!snippet.is_empty()).then_some(snippet),
            });
        }
        Ok(resp.text().await?)
    }
}
