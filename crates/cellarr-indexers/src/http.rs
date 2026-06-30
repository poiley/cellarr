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
