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
    /// The tag ids of the content this search is for. A tag-scoped indexer (one
    /// carrying [`tags`](cellarr_core::IndexerConfig::tags)) is included only when
    /// it shares a tag id here; an untagged indexer is global. Empty (the
    /// default) is the "no content tags" case — only global indexers apply, which
    /// matches today's behavior since indexers are untagged by default.
    content_tags: Vec<u32>,
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
            content_tags: Vec::new(),
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
            content_tags: Vec::new(),
        }
    }

    /// Scope this set to the tag ids of the content being searched, so a
    /// tag-scoped indexer is only fanned across when it shares a tag. Builder
    /// form; the default (no scoping) leaves only global indexers applying.
    #[must_use]
    pub fn with_content_tags(mut self, content_tags: Vec<u32>) -> Self {
        self.content_tags = content_tags;
        self
    }

    /// The enabled indexer configs the content's tags select, in configured
    /// priority order. A tag-scoped indexer is kept only when it shares a tag id
    /// with the content; an untagged indexer is global. With no content tags,
    /// only global (untagged) indexers are kept.
    async fn enabled_configs(&self) -> Result<Vec<IndexerConfig>, IndexerSetError> {
        let configs = self
            .db
            .config()
            .list_enabled_indexers()
            .await
            .map_err(IndexerSetError::Config)?;
        Ok(configs
            .into_iter()
            .filter(|ix| cellarr_core::tag_scope_applies(&ix.tags, &self.content_tags))
            .collect())
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
        let raw_base =
            setting_str(config, "baseUrl").ok_or_else(|| IndexerSetError::Misconfigured {
                name: config.name.clone(),
                reason: "missing baseUrl in settings".into(),
            })?;
        // Torznab/Newznab endpoints are `baseUrl` + `apiPath` (apiPath defaults to
        // "/api", the *arr convention). Prowlarr's app-sync stores baseUrl as
        // ".../{indexerId}/" with apiPath "/api"; without combining them we'd request
        // the bare ".../{indexerId}/" — Prowlarr's web UI — and get HTML back instead
        // of the caps XML, so every search fails.
        let base_url = combine_endpoint(&raw_base, setting_str(config, "apiPath").as_deref());
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

/// Combine an indexer's `baseUrl` with its `apiPath` into the full Torznab/Newznab
/// endpoint. `apiPath` defaults to `/api` (the *arr convention) and is appended to
/// the base unless the base already ends with it. Slashes are normalized so
/// `http://prowlarr/3/` + `/api` -> `http://prowlarr/3/api` (Prowlarr's app-sync
/// shape) and a host-only base `https://api.nzbgeek.info` -> `.../api`.
fn combine_endpoint(base_url: &str, api_path: Option<&str>) -> String {
    let base = base_url.trim_end_matches('/');
    let path = api_path
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or("/api");
    let path = format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    if base
        .to_ascii_lowercase()
        .ends_with(&path.to_ascii_lowercase())
    {
        base.to_string()
    } else {
        format!("{base}{path}")
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::IndexerId;
    use serde_json::json;

    #[test]
    fn combine_endpoint_joins_base_and_api_path() {
        // Prowlarr app-sync shape: base ".../{id}/" + apiPath "/api" -> ".../{id}/api"
        // (the bug was hitting the bare ".../{id}/", Prowlarr's UI, returning HTML).
        assert_eq!(
            combine_endpoint(
                "http://prowlarr.arr-stack.svc.cluster.local:9696/3/",
                Some("/api")
            ),
            "http://prowlarr.arr-stack.svc.cluster.local:9696/3/api"
        );
        // Host-only base + default apiPath.
        assert_eq!(
            combine_endpoint("https://api.nzbgeek.info", None),
            "https://api.nzbgeek.info/api"
        );
        // A base that already ends with the api path is not doubled.
        assert_eq!(
            combine_endpoint("https://tracker.example/torznab/api", Some("/api")),
            "https://tracker.example/torznab/api"
        );
        // Empty apiPath falls back to the default.
        assert_eq!(combine_endpoint("http://x/2/", Some("")), "http://x/2/api");
    }

    fn indexer(name: &str, tags: Vec<u32>) -> IndexerConfig {
        IndexerConfig {
            id: IndexerId::new(),
            name: name.into(),
            kind: "torznab".into(),
            protocol: Protocol::Torrent,
            enabled: true,
            priority: 25,
            criteria: Default::default(),
            tags,
            settings: json!({ "baseUrl": "http://localhost", "apiKey": "k" }),
        }
    }

    async fn temp_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("c.sqlite").to_str().unwrap())
            .await
            .unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn tag_scoped_indexer_excluded_for_non_matching_included_for_matching() {
        let (_dir, db) = temp_db().await;
        // A global (untagged) indexer and one scoped to tag 7.
        db.config()
            .upsert_indexer(&indexer("global", vec![]))
            .await
            .unwrap();
        db.config()
            .upsert_indexer(&indexer("scoped", vec![7]))
            .await
            .unwrap();

        // Content carrying tag 7: both apply.
        let set = DbIndexerSet::new(db.clone()).with_content_tags(vec![7]);
        let names: Vec<String> = set
            .enabled_configs()
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert!(names.contains(&"global".to_string()));
        assert!(names.contains(&"scoped".to_string()));

        // Content carrying tag 1 (not 7): the scoped indexer is excluded.
        let set = DbIndexerSet::new(db.clone()).with_content_tags(vec![1]);
        let names: Vec<String> = set
            .enabled_configs()
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, vec!["global".to_string()]);

        // Untagged content: only the global indexer is searched.
        let set = DbIndexerSet::new(db.clone()).with_content_tags(vec![]);
        let names: Vec<String> = set
            .enabled_configs()
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, vec!["global".to_string()]);
    }
}
