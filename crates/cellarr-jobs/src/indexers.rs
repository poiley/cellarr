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
use cellarr_indexers::{
    CardigannIndexer, Definition, FetcherPool, HostRateLimiter, IndexerError, NewznabIndexer,
    TorznabIndexer,
};

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
    /// Long-lived fetchers shared across searches. Adapters are rebuilt per search,
    /// but a FlareSolverr session must outlive them (see [`FetcherPool`]); sharing
    /// one pool the way the rate limiter is shared keeps a single session per
    /// indexer instead of standing up a rival browser for every search.
    fetchers: Arc<FetcherPool>,
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
            fetchers: Arc::new(FetcherPool::new()),
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
            fetchers: Arc::new(FetcherPool::new()),
        }
    }

    /// Use a caller-owned fetcher pool, so several per-run sets share one session
    /// per indexer. Without this each set builds its own pool, which is correct but
    /// gives up session reuse across runs.
    #[must_use]
    pub fn with_fetcher_pool(mut self, fetchers: Arc<FetcherPool>) -> Self {
        self.fetchers = fetchers;
        self
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
        // A Cardigann indexer carries a YAML definition rather than a Torznab
        // `baseUrl`; it is built from the definition's own `links`, so branch before
        // the `baseUrl` requirement below.
        if config.kind.eq_ignore_ascii_case("cardigann") {
            return self.build_cardigann(config);
        }

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

    /// Build a Cardigann adapter. The YAML definition comes from inline
    /// `settings.definition` or, failing that, the file at `settings.definitionFile`
    /// (so operators can keep a folder of definitions instead of pasting YAML). The
    /// remaining string settings become the `{{ .Config.* }}` template context (a
    /// passkey, a sitelink override, …).
    fn build_cardigann(&self, config: &IndexerConfig) -> Result<NabAdapter, IndexerSetError> {
        let yaml = match setting_str(config, "definition")
            .or_else(|| setting_str(config, "definitionYaml"))
        {
            Some(inline) => inline,
            None => {
                let path = setting_str(config, "definitionFile").ok_or_else(|| {
                    IndexerSetError::Misconfigured {
                        name: config.name.clone(),
                        reason: "missing cardigann 'definition' or 'definitionFile' in settings"
                            .into(),
                    }
                })?;
                std::fs::read_to_string(&path).map_err(|e| IndexerSetError::Misconfigured {
                    name: config.name.clone(),
                    reason: format!("reading cardigann definitionFile '{path}': {e}"),
                })?
            }
        };
        let definition =
            Definition::from_yaml(&yaml).map_err(|e| IndexerSetError::Misconfigured {
                name: config.name.clone(),
                reason: format!("invalid cardigann definition: {e}"),
            })?;
        let resolver = cellarr_indexers::resolver_for(&definition);
        let mut engine = CardigannIndexer::with_deps(
            config.id,
            definition,
            settings_string_map(config),
            self.cardigann_fetcher(config),
            Arc::clone(&self.rate_limiter),
        );
        if let Some(resolver) = resolver {
            engine = engine.with_resolver(resolver);
        }
        Ok(NabAdapter::Cardigann(engine))
    }

    /// The fetcher a Cardigann adapter uses.
    ///
    /// A tracker behind an interstitial bot check can't be reached by a plain HTTP
    /// client at all, so an operator points that indexer at a FlareSolverr instance
    /// with a `flaresolverrUrl` setting. It is per-indexer and absent by default:
    /// the single-binary/zero-required-services promise means no indexer may need
    /// an external process unless its operator opts in. The session is namespaced
    /// per indexer so two trackers never share clearance cookies.
    fn cardigann_fetcher(&self, config: &IndexerConfig) -> Arc<dyn cellarr_indexers::Fetcher> {
        match setting_str(config, "flaresolverrUrl").filter(|url| !url.trim().is_empty()) {
            Some(endpoint) => self
                .fetchers
                .flaresolverr(endpoint.trim(), &format!("cellarr-{}", config.id)),
            None => self.db_fetcher(),
        }
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
    /// A Cardigann-definition tracker (HTML-scrape, interpreted from YAML).
    Cardigann(CardigannIndexer),
}

impl NabAdapter {
    async fn search(&self, terms: &SearchTerms) -> cellarr_indexers::Result<Vec<Release>> {
        match self {
            NabAdapter::Torznab(a) => a.search(terms).await,
            NabAdapter::Newznab(a) => a.search(terms).await,
            NabAdapter::Cardigann(a) => a.search(terms).await,
        }
    }

    async fn latest(&self) -> cellarr_indexers::Result<Vec<Release>> {
        match self {
            NabAdapter::Torznab(a) => a.latest().await,
            NabAdapter::Newznab(a) => a.latest().await,
            NabAdapter::Cardigann(a) => a.latest().await,
        }
    }

    async fn resolve(&self, release: Release) -> cellarr_indexers::Result<Release> {
        match self {
            NabAdapter::Torznab(a) => a.resolve(release).await,
            NabAdapter::Newznab(a) => a.resolve(release).await,
            NabAdapter::Cardigann(a) => a.resolve(release).await,
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

/// Collect an indexer's string-valued settings into a `{key: value}` map — the
/// `{{ .Config.* }}` context a Cardigann definition templates against.
fn settings_string_map(config: &IndexerConfig) -> std::collections::BTreeMap<String, String> {
    config
        .settings
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
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

    /// Route a deferred-link resolve to the indexer that actually produced the
    /// release.
    ///
    /// The set fans searches out across every configured indexer, so a release
    /// carries the id of ONE of them. Inheriting the trait's default here — which
    /// returns the release untouched — meant a deferred link sailed through the set
    /// unresolved and reached the download client as the literal sentinel —
    /// `unsupported download_url scheme (not magnet: or http(s)): cellarr:deferred`.
    ///
    /// Fanning out is wrong for this: only the originating indexer holds the
    /// resolver and the session that can fetch the link, and asking the others would
    /// be pointless work against unrelated trackers.
    async fn resolve(&self, release: Release) -> Result<Release, Self::Error> {
        if !release.link_is_deferred() {
            return Ok(release);
        }
        let configs = self.enabled_configs().await?;
        let Some(config) = configs.iter().find(|c| c.id == release.indexer_id) else {
            return Err(IndexerSetError::Search {
                name: "configured-indexers".to_string(),
                source: cellarr_indexers::IndexerError::Unsupported(format!(
                    "no configured indexer {} to resolve this release's link",
                    release.indexer_id
                )),
            });
        };
        let name = config.name.clone();
        let adapter = self.build_adapter(config)?;
        adapter
            .resolve(release)
            .await
            .map_err(|source| IndexerSetError::Search { name, source })
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

    pub(super) async fn temp_db() -> (tempfile::TempDir, Database) {
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

    /// A minimal but complete Cardigann definition for the wiring test.
    const CARDIGANN_DEF: &str = r#"
id: wiring
name: Wiring Tracker
links: [https://wiring.example/]
search:
  paths:
    - path: /s
      inputs: { q: "{{ .Keywords }}" }
  rows:
    selector: tr
  fields:
    title:
      selector: a
    download:
      selector: a
      attribute: href
"#;

    #[tokio::test]
    async fn build_adapter_builds_a_cardigann_indexer_from_its_definition() {
        let (_dir, db) = temp_db().await;
        let set = DbIndexerSet::new(db);
        let config = IndexerConfig {
            id: IndexerId::new(),
            name: "configured name".into(),
            kind: "cardigann".into(),
            protocol: Protocol::Torrent,
            enabled: true,
            priority: 25,
            criteria: Default::default(),
            tags: vec![],
            settings: json!({ "definition": CARDIGANN_DEF }),
        };

        // kind=cardigann -> a Cardigann adapter whose name comes from the parsed
        // definition (not the config), proving the YAML was interpreted.
        match set.build_adapter(&config).expect("build cardigann adapter") {
            NabAdapter::Cardigann(a) => assert_eq!(a.name(), "Wiring Tracker"),
            _ => panic!("expected a Cardigann adapter"),
        }

        // A cardigann indexer without a definition is a clear misconfiguration,
        // surfaced (and skipped) rather than panicking.
        let missing = IndexerConfig {
            settings: json!({}),
            ..config
        };
        assert!(matches!(
            set.build_adapter(&missing),
            Err(IndexerSetError::Misconfigured { .. })
        ));
    }

    /// A tracker behind a bot check is unreachable by a plain client, so the
    /// `flaresolverrUrl` setting has to actually change which fetcher the adapter
    /// gets. Asserted by watching what arrives on the wire: a FlareSolverr envelope
    /// rather than a direct GET of the tracker.
    #[tokio::test]
    async fn a_flaresolverr_url_setting_routes_the_adapter_through_it() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let recorder = Arc::clone(&seen);
        let server = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let recorder = Arc::clone(&recorder);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    recorder
                        .lock()
                        .expect("lock")
                        .push(String::from_utf8_lossy(&buf[..n]).to_string());
                    // A FlareSolverr envelope wrapping an empty listing.
                    let body = r#"{"status":"ok","solution":{"status":200,"response":"<html><body><table></table></body></html>"}}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });

        let (_dir, db) = temp_db().await;
        let set = DbIndexerSet::new(db);
        let config = IndexerConfig {
            id: IndexerId::new(),
            name: "protected".into(),
            kind: "cardigann".into(),
            protocol: Protocol::Torrent,
            enabled: true,
            priority: 25,
            criteria: Default::default(),
            tags: vec![],
            settings: json!({
                "definition": CARDIGANN_DEF,
                "flaresolverrUrl": format!("http://{addr}"),
            }),
        };

        let adapter = set.build_adapter(&config).expect("build cardigann adapter");
        let NabAdapter::Cardigann(engine) = adapter else {
            panic!("expected a Cardigann adapter");
        };
        let _ = engine.latest().await;
        server.abort();

        let requests = seen.lock().expect("lock").clone();
        assert!(
            !requests.is_empty(),
            "nothing reached the flaresolverr stub"
        );
        let first = &requests[0];
        assert!(
            first.contains("POST /v1"),
            "expected a flaresolverr command, got: {first}"
        );
        assert!(
            first.contains("sessions.list")
                || first.contains("sessions.create")
                || first.contains("request.get"),
            "expected a flaresolverr envelope, got: {first}"
        );
    }

    /// A FlareSolverr session is one browser and must outlive the per-search
    /// adapters. Two adapters built for the same indexer have to land on the same
    /// pooled fetcher, or each search stands up a rival session and they crash each
    /// other's tabs under load.
    #[tokio::test]
    async fn adapters_for_one_indexer_share_a_pooled_fetcher() {
        let (_dir, db) = temp_db().await;
        let pool = Arc::new(FetcherPool::new());
        let set = DbIndexerSet::new(db).with_fetcher_pool(Arc::clone(&pool));
        let config = IndexerConfig {
            id: IndexerId::new(),
            name: "protected".into(),
            kind: "cardigann".into(),
            protocol: Protocol::Torrent,
            enabled: true,
            priority: 25,
            criteria: Default::default(),
            tags: vec![],
            settings: json!({
                "definition": CARDIGANN_DEF,
                "flaresolverrUrl": "http://flaresolverr.invalid:8191",
            }),
        };

        let first = set.cardigann_fetcher(&config);
        let second = set.cardigann_fetcher(&config);
        assert!(
            Arc::ptr_eq(&first, &second),
            "the same indexer must reuse one pooled fetcher"
        );

        // A different indexer id is a different session, so a different fetcher.
        let other = IndexerConfig {
            id: IndexerId::new(),
            ..config
        };
        assert!(
            !Arc::ptr_eq(&first, &set.cardigann_fetcher(&other)),
            "distinct indexers must not share a session"
        );
    }

    /// Without the setting the adapter keeps the plain fetcher, so an ordinary
    /// tracker never depends on an external process.
    #[tokio::test]
    async fn without_the_setting_no_flaresolverr_is_used() {
        let (_dir, db) = temp_db().await;
        let set = DbIndexerSet::new(db);
        let config = IndexerConfig {
            id: IndexerId::new(),
            name: "plain".into(),
            kind: "cardigann".into(),
            protocol: Protocol::Torrent,
            enabled: true,
            priority: 25,
            criteria: Default::default(),
            tags: vec![],
            // An empty value is treated as absent rather than as an endpoint.
            settings: json!({ "definition": CARDIGANN_DEF, "flaresolverrUrl": "  " }),
        };
        // Building succeeds and yields a Cardigann adapter; the fetcher choice is
        // observable in the sibling test, and here only that it did not error.
        assert!(matches!(
            set.build_adapter(&config).expect("build"),
            NabAdapter::Cardigann(_)
        ));
    }

    #[tokio::test]
    async fn build_adapter_reads_a_cardigann_definition_from_a_file() {
        let (_dir, db) = temp_db().await;
        let set = DbIndexerSet::new(db);
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("wiring.yml");
        std::fs::write(&path, CARDIGANN_DEF).unwrap();

        let config = IndexerConfig {
            id: IndexerId::new(),
            name: "file-sourced".into(),
            kind: "cardigann".into(),
            protocol: Protocol::Torrent,
            enabled: true,
            priority: 25,
            criteria: Default::default(),
            tags: vec![],
            settings: json!({ "definitionFile": path.to_str().unwrap() }),
        };

        // The definition is loaded from the file path in settings.
        match set.build_adapter(&config).expect("build from file") {
            NabAdapter::Cardigann(a) => assert_eq!(a.name(), "Wiring Tracker"),
            _ => panic!("expected a Cardigann adapter"),
        }

        // A non-existent file is a clear misconfiguration.
        let bad = IndexerConfig {
            settings: json!({ "definitionFile": "/no/such/definition.yml" }),
            ..config
        };
        assert!(matches!(
            set.build_adapter(&bad),
            Err(IndexerSetError::Misconfigured { .. })
        ));
    }
}

#[cfg(test)]
mod deferred_resolve_routing_tests {
    use super::*;
    use cellarr_core::{DEFERRED_LINK, IndexerId, Protocol};

    fn deferred_release(indexer_id: IndexerId) -> Release {
        Release {
            indexer_id,
            title: "Some.Release.1080p".to_string(),
            download_url: DEFERRED_LINK.to_string(),
            guid: Some("https://tracker.example/some-release-1/".to_string()),
            protocol: Protocol::Torrent,
            size: Some(1),
            seeders: Some(9),
            indexer_flags: vec![],
        }
    }

    /// A release whose link is deferred can only be resolved by the indexer that
    /// produced it — that is where the resolver and the authenticated session live.
    ///
    /// Inheriting the trait's default no-op here let the sentinel sail through the
    /// set unresolved and reach the download client verbatim:
    /// `unsupported download_url scheme (not magnet: or http(s)): cellarr:deferred`.
    /// 26 grabs failed that way within an hour of shipping deferred links.
    #[tokio::test]
    async fn a_deferred_release_from_an_unknown_indexer_is_an_error_not_a_silent_pass() {
        let (_dir, db) = crate::indexers::tests::temp_db().await;
        let set = DbIndexerSet::new(db);

        // No indexer configured with this id: resolving is impossible, and saying so
        // is the point — passing the sentinel on would fail at the download client
        // with a message about the URL scheme, which explains nothing.
        let orphan = deferred_release(IndexerId::new());
        let err = Indexer::resolve(&set, orphan)
            .await
            .expect_err("an unresolvable deferred link must be an error");
        assert!(
            err.to_string().contains("resolve"),
            "the error must name what could not be done, got: {err}"
        );
    }

    /// A release that already carries a real link is returned untouched, without
    /// consulting any indexer — resolving is only for deferred links.
    #[tokio::test]
    async fn a_release_with_a_real_link_is_returned_untouched() {
        let (_dir, db) = crate::indexers::tests::temp_db().await;
        let set = DbIndexerSet::new(db);

        let mut ready = deferred_release(IndexerId::new());
        ready.download_url = "magnet:?xt=urn:btih:abc".to_string();
        let out = Indexer::resolve(&set, ready.clone())
            .await
            .expect("a resolved release needs no indexer");
        assert_eq!(out.download_url, ready.download_url);
    }
}
