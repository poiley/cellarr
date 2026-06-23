//! The live discovery seam: build indexer adapters from persisted configuration
//! and fan a search across them.
//!
//! Phase A persisted [`IndexerConfig`] rows (URL, API key, categories, protocol)
//! via the db `ConfigRepo`; the pipeline's Discover stage takes a single
//! [`cellarr_core::Indexer`]. [`DbIndexerSet`] bridges the two: it reads the
//! *enabled* indexer configs at search time, constructs the matching native
//! [`TorznabIndexer`]/[`NewznabIndexer`] adapter for each (which itself calls
//! `t=caps` first, then the typed search), normalizes every result into
//! [`Release`], and concatenates them in configured priority order. The runner is
//! then driven over this one aggregate seam unchanged.
//!
//! Reading the config *per search* (rather than caching adapters) keeps the live
//! set in step with CRUD writes: an indexer added or removed through the API is
//! visible to the very next pipeline run with no restart. Capabilities are still
//! cached per-adapter for the lifetime of one search call, so a fan-out issues
//! `t=caps` at most once per indexer per search.

use std::sync::Arc;

use async_trait::async_trait;
use cellarr_core::{Indexer, IndexerConfig, Protocol, Release, SearchTerms};
use cellarr_db::Database;
use cellarr_indexers::{HostRateLimiter, IndexerError, NewznabIndexer, TorznabIndexer};

/// A failure building or fanning out the configured indexer set.
#[derive(Debug, thiserror::Error)]
pub enum IndexerSetError {
    /// Reading the persisted indexer configuration failed.
    #[error("reading indexer configuration failed: {0}")]
    Config(#[source] cellarr_db::DbError),

    /// A configured indexer's settings were missing or malformed (e.g. no
    /// `baseUrl`), so no adapter could be built from it.
    #[error("indexer '{name}' is misconfigured: {reason}")]
    Misconfigured {
        /// The configured indexer's name.
        name: String,
        /// Why the adapter could not be built.
        reason: String,
    },

    /// A configured adapter failed during caps/search. Carries the indexer name
    /// so the decision log records *which* indexer broke.
    #[error("indexer '{name}' search failed: {source}")]
    Search {
        /// The configured indexer's name.
        name: String,
        /// The underlying adapter error (a banned key, a parse failure, …).
        #[source]
        source: IndexerError,
    },
}

/// An aggregate [`Indexer`] backed by the persisted indexer configuration.
///
/// Clone is cheap (it holds a [`Database`] handle and a shared rate limiter).
#[derive(Clone)]
pub struct DbIndexerSet {
    db: Database,
    /// Shared, per-host rate limiter so indexers on the same tracker host share
    /// the budget the tracker enforces across every search.
    rate_limiter: Arc<HostRateLimiter>,
    /// If true, a single indexer failing aborts the whole search; if false
    /// (default) a failing indexer is skipped and the rest still contribute.
    fail_fast: bool,
}

impl DbIndexerSet {
    /// Build a set over the database's indexer configuration with a conservative
    /// shared rate limiter. Failing indexers are skipped (best-effort fan-out).
    #[must_use]
    pub fn new(db: Database) -> Self {
        Self {
            db,
            rate_limiter: Arc::new(HostRateLimiter::conservative_default()),
            fail_fast: false,
        }
    }

    /// Build a set with an explicit shared rate limiter (so several seams can
    /// share one host budget) and fail-fast policy.
    #[must_use]
    pub fn with_rate_limiter(
        db: Database,
        rate_limiter: Arc<HostRateLimiter>,
        fail_fast: bool,
    ) -> Self {
        Self {
            db,
            rate_limiter,
            fail_fast,
        }
    }

    /// The enabled indexer configs, in configured priority order.
    async fn enabled_configs(&self) -> Result<Vec<IndexerConfig>, IndexerSetError> {
        self.db
            .config()
            .list_enabled_indexers()
            .await
            .map_err(IndexerSetError::Config)
    }

    /// Run `op` against every enabled, well-formed adapter, concatenating the
    /// releases. Misconfigured indexers and (unless `fail_fast`) adapter failures
    /// are skipped so one bad indexer never blinds discovery.
    async fn fan_out<F, Fut>(&self, op: F) -> Result<Vec<Release>, IndexerSetError>
    where
        F: Fn(NabAdapter) -> Fut,
        Fut: std::future::Future<Output = cellarr_indexers::Result<Vec<Release>>>,
    {
        let configs = self.enabled_configs().await?;
        let mut all = Vec::new();
        for config in configs {
            let adapter = match self.build_adapter(&config) {
                Ok(a) => a,
                Err(e) if self.fail_fast => return Err(e),
                Err(e) => {
                    tracing::warn!(indexer = %config.name, error = %e, "skipping misconfigured indexer");
                    continue;
                }
            };
            match op(adapter).await {
                Ok(mut releases) => all.append(&mut releases),
                Err(source) if self.fail_fast => {
                    return Err(IndexerSetError::Search {
                        name: config.name,
                        source,
                    });
                }
                Err(source) => {
                    tracing::warn!(indexer = %config.name, error = %source, "indexer search failed; skipping");
                }
            }
        }
        Ok(all)
    }

    /// Construct the native adapter for one config, reading `baseUrl`/`apiKey`
    /// from its open-ended `settings` JSON (the shape the API shim persists).
    fn build_adapter(&self, config: &IndexerConfig) -> Result<NabAdapter, IndexerSetError> {
        let base_url =
            setting_str(config, "baseUrl").ok_or_else(|| IndexerSetError::Misconfigured {
                name: config.name.clone(),
                reason: "missing baseUrl in settings".into(),
            })?;
        let api_key = setting_str(config, "apiKey").filter(|k| !k.is_empty());

        let is_newznab =
            config.kind.eq_ignore_ascii_case("newznab") || config.protocol == Protocol::Usenet;

        let build = |proto_torznab: bool| -> cellarr_indexers::Result<NabAdapter> {
            if proto_torznab {
                Ok(NabAdapter::Torznab(TorznabIndexer::with_deps(
                    config.id,
                    config.name.clone(),
                    &base_url,
                    api_key.clone(),
                    self.db_fetcher(),
                    Arc::clone(&self.rate_limiter),
                )?))
            } else {
                Ok(NabAdapter::Newznab(NewznabIndexer::with_deps(
                    config.id,
                    config.name.clone(),
                    &base_url,
                    api_key.clone(),
                    self.db_fetcher(),
                    Arc::clone(&self.rate_limiter),
                )?))
            }
        };

        build(!is_newznab).map_err(|source| IndexerSetError::Search {
            name: config.name.clone(),
            source,
        })
    }

    /// The HTTP fetcher used by built adapters: a real `reqwest` fetcher, so the
    /// fan-out makes genuine HTTP requests to each indexer's `baseUrl`. Tests
    /// exercise it against a local HTTP server bound to `127.0.0.1`.
    fn db_fetcher(&self) -> Arc<dyn cellarr_indexers::Fetcher> {
        Arc::new(cellarr_indexers::ReqwestFetcher::default())
    }
}

/// One built native adapter, dispatched dynamically by protocol.
pub enum NabAdapter {
    /// A Torznab (torrent) adapter.
    Torznab(TorznabIndexer),
    /// A Newznab (usenet) adapter.
    Newznab(NewznabIndexer),
}

impl NabAdapter {
    async fn search(&self, terms: &SearchTerms) -> cellarr_indexers::Result<Vec<Release>> {
        match self {
            NabAdapter::Torznab(a) => a.search(terms).await,
            NabAdapter::Newznab(a) => a.search(terms).await,
        }
    }

    async fn latest(&self) -> cellarr_indexers::Result<Vec<Release>> {
        match self {
            NabAdapter::Torznab(a) => a.latest().await,
            NabAdapter::Newznab(a) => a.latest().await,
        }
    }
}

/// Read a string setting out of an [`IndexerConfig`]'s `settings` JSON object.
fn setting_str(config: &IndexerConfig, key: &str) -> Option<String> {
    config
        .settings
        .as_object()
        .and_then(|o| o.get(key))
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

#[async_trait]
impl Indexer for DbIndexerSet {
    type Error = IndexerSetError;

    fn name(&self) -> &str {
        "configured-indexers"
    }

    async fn search(&self, terms: &SearchTerms) -> Result<Vec<Release>, Self::Error> {
        self.fan_out(|adapter| {
            let terms = terms.clone();
            async move { adapter.search(&terms).await }
        })
        .await
    }

    async fn latest(&self) -> Result<Vec<Release>, Self::Error> {
        self.fan_out(|adapter| async move { adapter.latest().await })
            .await
    }
}
