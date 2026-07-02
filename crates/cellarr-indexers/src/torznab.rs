//! Native Torznab / Newznab indexer adapters.
//!
//! Both protocols are near-identical HTTP APIs returning RSS/XML; the only real
//! differences are the download protocol of the results and small defaults. They
//! share one implementation ([`NabIndexer`]) parameterized by [`Protocol`];
//! [`TorznabIndexer`] and [`NewznabIndexer`] are thin constructors.
//!
//! Per `docs/06-integrations.md` the adapter **calls `t=caps` first** and reads
//! the advertised modes/params from it — it never hardcodes categories or assumes
//! a param is supported.

use std::sync::Arc;

use async_trait::async_trait;
use cellarr_core::{Indexer, IndexerId, Protocol, Release, SearchTerms};
use tokio::sync::OnceCell;
use url::Url;

use crate::caps::{parse_caps, Caps};
use crate::error::{IndexerError, Result};
use crate::feed::parse_feed;
use crate::http::{Fetcher, ReqwestFetcher};
use crate::ratelimit::HostRateLimiter;

/// A native Torznab/Newznab adapter.
pub struct NabIndexer {
    id: IndexerId,
    name: String,
    /// Base API URL, e.g. `https://indexer.example/api`.
    base_url: Url,
    /// API key sent as the `apikey` query param (Newznab/Torznab convention).
    api_key: Option<String>,
    protocol: Protocol,
    fetcher: Arc<dyn Fetcher>,
    rate_limiter: Arc<HostRateLimiter>,
    /// Lazily-fetched, cached capabilities — `t=caps` is called once.
    caps: OnceCell<Caps>,
}

impl NabIndexer {
    /// Construct an adapter. `base_url` is the indexer's API endpoint.
    pub fn new(
        id: IndexerId,
        name: impl Into<String>,
        base_url: &str,
        api_key: Option<String>,
        protocol: Protocol,
        fetcher: Arc<dyn Fetcher>,
        rate_limiter: Arc<HostRateLimiter>,
    ) -> Result<Self> {
        Ok(Self {
            id,
            name: name.into(),
            base_url: Url::parse(base_url)?,
            api_key,
            protocol,
            fetcher,
            rate_limiter,
            caps: OnceCell::new(),
        })
    }

    /// Build a request URL for a given `t=` mode plus extra `(key, value)` params.
    ///
    /// The `apikey` is appended when configured. Params are added verbatim; the
    /// caller is responsible for only passing params caps advertises.
    fn build_url(&self, mode: &str, params: &[(String, String)]) -> Url {
        let mut url = self.base_url.clone();
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("t", mode);
            if let Some(key) = &self.api_key {
                qp.append_pair("apikey", key);
            }
            for (k, v) in params {
                qp.append_pair(k, v);
            }
        }
        url
    }

    /// The host used for rate-limit keying.
    fn host(&self) -> &str {
        self.base_url.host_str().unwrap_or("")
    }

    /// Fetch and cache capabilities (`t=caps`). Called before any search.
    pub async fn caps(&self) -> Result<&Caps> {
        self.caps
            .get_or_try_init(|| async {
                let url = self.build_url("caps", &[]);
                self.rate_limiter.until_ready(self.host()).await;
                let body = self.fetcher.get(url.as_str()).await?;
                parse_caps(&body)
            })
            .await
    }

    /// Choose the most specific advertised mode for the given terms.
    ///
    /// We prefer a typed mode (`tvsearch`/`movie`) when the terms carry the IDs
    /// it would use and caps advertises it; otherwise fall back to plain
    /// `search`. We never return a mode caps does not advertise.
    fn select_mode<'a>(&self, caps: &'a Caps, terms: &SearchTerms) -> Result<&'a str> {
        let has_id = |keys: &[&str]| {
            terms
                .ids
                .iter()
                .any(|(k, _)| keys.iter().any(|wanted| k == wanted))
        };

        // tv ids/numbering -> tvsearch; movie ids -> movie.
        if (has_id(&["tvdbid", "rid", "tvmazeid"]) || !terms.numbering.is_empty())
            && caps.has_mode("tvsearch")
        {
            return Ok("tvsearch");
        }
        if has_id(&["imdbid", "tmdbid"]) && caps.has_mode("movie") {
            return Ok("movie");
        }
        if caps.has_mode("search") {
            return Ok("search");
        }
        Err(IndexerError::UnsupportedMode("search".to_string()))
    }

    /// Assemble the query params for `mode` from `terms`, dropping any param the
    /// caps document does not list as supported for that mode.
    fn build_params(caps: &Caps, mode_name: &str, terms: &SearchTerms) -> Vec<(String, String)> {
        let mode = caps.mode(mode_name);
        let allowed = |param: &str| mode.is_none_or(|m| m.supports_param(param));

        let mut params = Vec::new();

        // `q` is the free-text query. Join queries with spaces, most-specific
        // first; callers can also issue multiple searches if they prefer.
        if let Some(q) = terms.queries.first() {
            if allowed("q") && !q.is_empty() {
                params.push(("q".to_string(), q.clone()));
            }
        }
        for (k, v) in &terms.ids {
            if allowed(k) {
                params.push((k.clone(), v.clone()));
            }
        }
        for (k, v) in &terms.numbering {
            if allowed(k) {
                params.push((k.clone(), v.clone()));
            }
        }
        params
    }

    /// Run a search for a specific `t=` mode (used by both `search` and `latest`).
    async fn run_search(&self, mode: &str, params: &[(String, String)]) -> Result<Vec<Release>> {
        let url = self.build_url(mode, params);
        self.rate_limiter.until_ready(self.host()).await;
        let body = self.fetcher.get(url.as_str()).await?;
        parse_feed(&body, self.id, self.protocol)
    }
}

#[async_trait]
impl Indexer for NabIndexer {
    type Error = IndexerError;

    fn name(&self) -> &str {
        &self.name
    }

    #[tracing::instrument(name = "indexer.search", skip_all, fields(indexer = %self.name))]
    async fn search(&self, terms: &SearchTerms) -> Result<Vec<Release>> {
        let caps = self.caps().await?.clone();
        let mode = self.select_mode(&caps, terms)?;
        let params = Self::build_params(&caps, mode, terms);
        self.run_search(mode, &params).await
    }

    async fn latest(&self) -> Result<Vec<Release>> {
        // RSS-style discovery: plain `search` with no query returns the newest
        // releases. Only issue it if caps advertises the mode.
        let caps = self.caps().await?.clone();
        if !caps.has_mode("search") {
            return Err(IndexerError::UnsupportedMode("search".to_string()));
        }
        self.run_search("search", &[]).await
    }
}

/// A native Torznab (torrent) indexer.
pub struct TorznabIndexer(NabIndexer);

impl TorznabIndexer {
    /// Construct a Torznab adapter using the default `reqwest` fetcher and a
    /// conservative shared rate limiter.
    pub fn new(
        id: IndexerId,
        name: impl Into<String>,
        base_url: &str,
        api_key: Option<String>,
    ) -> Result<Self> {
        Self::with_deps(
            id,
            name,
            base_url,
            api_key,
            Arc::new(ReqwestFetcher::default()),
            Arc::new(HostRateLimiter::conservative_default()),
        )
    }

    /// Construct with explicit dependencies (used by tests for record/replay).
    pub fn with_deps(
        id: IndexerId,
        name: impl Into<String>,
        base_url: &str,
        api_key: Option<String>,
        fetcher: Arc<dyn Fetcher>,
        rate_limiter: Arc<HostRateLimiter>,
    ) -> Result<Self> {
        Ok(Self(NabIndexer::new(
            id,
            name,
            base_url,
            api_key,
            Protocol::Torrent,
            fetcher,
            rate_limiter,
        )?))
    }

    /// Fetch & cache capabilities.
    pub async fn caps(&self) -> Result<&Caps> {
        self.0.caps().await
    }
}

#[async_trait]
impl Indexer for TorznabIndexer {
    type Error = IndexerError;
    fn name(&self) -> &str {
        self.0.name()
    }
    async fn search(&self, terms: &SearchTerms) -> Result<Vec<Release>> {
        self.0.search(terms).await
    }
    async fn latest(&self) -> Result<Vec<Release>> {
        self.0.latest().await
    }
}

/// A native Newznab (Usenet) indexer.
pub struct NewznabIndexer(NabIndexer);

impl NewznabIndexer {
    /// Construct a Newznab adapter using the default `reqwest` fetcher and a
    /// conservative shared rate limiter.
    pub fn new(
        id: IndexerId,
        name: impl Into<String>,
        base_url: &str,
        api_key: Option<String>,
    ) -> Result<Self> {
        Self::with_deps(
            id,
            name,
            base_url,
            api_key,
            Arc::new(ReqwestFetcher::default()),
            Arc::new(HostRateLimiter::conservative_default()),
        )
    }

    /// Construct with explicit dependencies (used by tests for record/replay).
    pub fn with_deps(
        id: IndexerId,
        name: impl Into<String>,
        base_url: &str,
        api_key: Option<String>,
        fetcher: Arc<dyn Fetcher>,
        rate_limiter: Arc<HostRateLimiter>,
    ) -> Result<Self> {
        Ok(Self(NabIndexer::new(
            id,
            name,
            base_url,
            api_key,
            Protocol::Usenet,
            fetcher,
            rate_limiter,
        )?))
    }

    /// Fetch & cache capabilities.
    pub async fn caps(&self) -> Result<&Caps> {
        self.0.caps().await
    }
}

#[async_trait]
impl Indexer for NewznabIndexer {
    type Error = IndexerError;
    fn name(&self) -> &str {
        self.0.name()
    }
    async fn search(&self, terms: &SearchTerms) -> Result<Vec<Release>> {
        self.0.search(terms).await
    }
    async fn latest(&self) -> Result<Vec<Release>> {
        self.0.latest().await
    }
}
