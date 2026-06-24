//! The live pipeline handler: the seam that finally connects the **scheduler** to
//! the **runner**.
//!
//! `cellarr-jobs`' [`Scheduler`](cellarr_jobs::Scheduler) decides *when* a job
//! runs; its [`PipelineRunner`](cellarr_jobs::PipelineRunner) does the *work* of
//! one acquisition (Discover→…→Import). Until now the daemon wired the scheduler
//! to `cellarr-api`'s event-only `CommandHandler`, which published a domain event
//! and did **no** search/grab/import — the runner was never driven by the running
//! daemon. [`LivePipelineHandler`] closes that gap: it is the
//! [`JobHandler`](cellarr_jobs::JobHandler) the daemon injects, and on each fired
//! job it loads the relevant content from the DB, builds the configured indexer
//! set + download client, and drives the real [`PipelineRunner`] end to end,
//! publishing the matching [`DomainEvent`]s onto the shared [`EventBus`] so the
//! UI/WS still observe progress.
//!
//! ## What each job kind drives
//!
//! - [`MissingItemSearch`](JobKind::MissingItemSearch): load **monitored-missing**
//!   leaf content (bounded), run a full search for each through the runner.
//! - [`ManualSearch`](JobKind::ManualSearch): the same, for one named content node.
//! - [`RssSync`](JobKind::RssSync): poll the indexers' RSS (`latest`) the same way,
//!   driving each monitored-missing node through the runner (the runner's Discover
//!   uses the indexer's `search`, so RSS here re-uses the search path bounded to
//!   the monitored set rather than a tight per-feed loop).
//! - other kinds (metadata refresh, disk check) are not pipeline work here and are
//!   reported as a benign success.
//!
//! ## Environment seam
//!
//! The handler is generic over a [`PipelineEnv`]: the source of the live indexer,
//! the live download client, and the per-run [`RunnerConfig`]. The daemon binds
//! [`LivePipelineEnv`] (DB-backed: [`DbIndexerSet`] +
//! [`ConfiguredDownloadClient`](crate::clients::ConfiguredDownloadClient) + a
//! config resolved from libraries/profiles/remote-path mappings). Tests bind a
//! fake env with a fake indexer + fake client + a temp library root, so the whole
//! scheduler→handler→runner→import chain is exercised offline with no Docker.

use std::sync::Arc;

use async_trait::async_trait;

use cellarr_api::events::{DomainEvent, EventBus};
use cellarr_core::repo::{ContentRepository, ProfileRepository};
use cellarr_core::{ContentRef, DownloadClient, Indexer, MediaType, QualityRanking};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_indexers::HostRateLimiter;
use cellarr_jobs::clock::SystemClock;
use cellarr_jobs::runner::{PipelineRunner, RunOutcome, RunnerConfig};
use cellarr_jobs::{DbIndexerSet, JobHandler, JobKind, JobResult};
use cellarr_media::MediaRegistry;

use crate::clients::{ConfiguredDownloadClient, NoopDownloadClient};

/// The maximum number of monitored-missing nodes one `MissingItemSearch` /
/// `RssSync` tick drives through the pipeline, so a large backlog never makes a
/// single job run unboundedly. The remainder is picked up on the next tick.
const MAX_NODES_PER_RUN: usize = 50;

/// The live seams one pipeline run needs, resolved freshly per run so CRUD writes
/// (a new indexer, a reconfigured client) take effect with no restart.
///
/// Implemented by [`LivePipelineEnv`] (the daemon, DB-backed) and by a fake in
/// tests. Returning the seams by value keeps the runner's borrow scope local to a
/// single run.
#[async_trait]
pub trait PipelineEnv: Send + Sync {
    /// The aggregate indexer the runner's Discover stage searches across.
    type Indexer: Indexer;
    /// The download client the runner's Grab/Track stages drive.
    type Client: DownloadClient;

    /// Build the indexer + download client + runner config for `content`.
    ///
    /// `content` is provided so the config can be scoped (library root / profile)
    /// to the node being acquired. Returns `Ok(None)` when the environment is not
    /// ready to run (e.g. no enabled download client configured) — a benign,
    /// logged no-op rather than an error.
    async fn resolve(
        &self,
        content: &ContentRef,
    ) -> Result<Option<(Self::Indexer, Self::Client, RunnerConfig)>, String>;
}

/// The [`JobHandler`] the daemon injects: drives the real pipeline on every fired
/// job and publishes domain events onto the shared bus.
pub struct LivePipelineHandler<E: PipelineEnv> {
    db: Database,
    registry: Arc<MediaRegistry>,
    events: EventBus,
    env: E,
    clock: SystemClock,
}

impl<E: PipelineEnv> LivePipelineHandler<E> {
    /// Assemble the handler from the persistence handle, the media registry, the
    /// shared event bus, and the environment seam.
    pub fn new(db: Database, registry: Arc<MediaRegistry>, events: EventBus, env: E) -> Self {
        Self {
            db,
            registry,
            events,
            env,
            clock: SystemClock,
        }
    }

    /// Load the bounded set of monitored-missing leaf nodes to acquire.
    async fn missing_nodes(&self) -> Result<Vec<ContentRef>, String> {
        let mut nodes = self
            .db
            .content()
            .monitored_missing()
            .await
            .map_err(|e| format!("loading monitored-missing content failed: {e}"))?;
        nodes.truncate(MAX_NODES_PER_RUN);
        Ok(nodes)
    }

    /// Load one specific content node for a manual search.
    async fn one_node(&self, content_id: &str) -> Result<Option<ContentRef>, String> {
        let id = content_id
            .parse::<uuid::Uuid>()
            .map(cellarr_core::ContentId::from_uuid)
            .map_err(|e| format!("invalid content id '{content_id}': {e}"))?;
        self.db
            .content()
            .get(id)
            .await
            .map_err(|e| format!("loading content {content_id} failed: {e}"))
    }

    /// Drive one content node through the full runner, publishing the matching
    /// domain events. Returns `true` if a run actually executed (the environment
    /// was ready), `false` if it was a no-op (no client configured).
    async fn run_node(&self, content: &ContentRef) -> Result<bool, String> {
        let Some((indexer, client, config)) = self.env.resolve(content).await? else {
            return Ok(false);
        };
        let runner = PipelineRunner::new(
            &indexer,
            &client,
            &self.registry,
            &self.db,
            &self.clock,
            &config,
        );
        let outcome = runner
            .run(content)
            .await
            .map_err(|e| format!("pipeline run failed: {e}"))?;
        self.publish_outcome(content, &outcome);
        Ok(true)
    }

    /// Translate a terminal [`RunOutcome`] into the live [`DomainEvent`]s the
    /// UI/WS subscribe to. The runner already wrote the authoritative decision
    /// log + history; this surfaces the same transition onto the push bus.
    fn publish_outcome(&self, content: &ContentRef, outcome: &RunOutcome) {
        match outcome {
            RunOutcome::Imported {
                grab_id,
                destinations,
            } => {
                self.events.publish(DomainEvent::QueueProgress {
                    grab_id: grab_id.to_string(),
                    status: "imported".to_string(),
                    progress: Some(1.0),
                });
                for path in destinations {
                    self.events.publish(DomainEvent::ImportCompleted {
                        content_id: content.id.to_string(),
                        path: path.clone(),
                    });
                }
            }
            RunOutcome::Rejected { reason } => {
                self.events.publish(DomainEvent::DecisionLogged {
                    run_id: content.id.to_string(),
                    note: format!("rejected: {reason}"),
                });
            }
            RunOutcome::Failed { detail } => {
                self.events.publish(DomainEvent::DecisionLogged {
                    run_id: content.id.to_string(),
                    note: format!("grab failed: {detail}"),
                });
            }
            RunOutcome::HeldForReview { reason } => {
                self.events.publish(DomainEvent::DecisionLogged {
                    run_id: content.id.to_string(),
                    note: format!("held for review: {reason}"),
                });
            }
            RunOutcome::NothingFound => {
                self.events.publish(DomainEvent::DecisionLogged {
                    run_id: content.id.to_string(),
                    note: "no releases found".to_string(),
                });
            }
        }
    }

    /// Drive every monitored-missing node (bounded) through the runner. Used by
    /// both `MissingItemSearch` and `RssSync` — both want to acquire the gaps; the
    /// runner's Discover handles RSS-vs-search at the indexer level.
    async fn run_missing(&self) -> JobResult {
        let nodes = match self.missing_nodes().await {
            Ok(n) => n,
            Err(detail) => return JobResult::Retryable { detail },
        };
        for node in &nodes {
            // One node failing (a flaky indexer, a transient write) must not abort
            // the whole sweep; record it and move on. The next tick retries the
            // still-missing nodes.
            if let Err(detail) = self.run_node(node).await {
                tracing::warn!(content = %node.id, error = %detail, "pipeline run failed for node; continuing");
            }
        }
        JobResult::Success
    }
}

#[async_trait]
impl<E: PipelineEnv> JobHandler for LivePipelineHandler<E> {
    async fn handle(&self, kind: &JobKind) -> JobResult {
        // Surface the command on the bus first (parity with the event-only
        // handler), then do the real work.
        self.events.publish(DomainEvent::CommandQueued {
            job_id: String::new(),
            name: command_label(kind).to_string(),
        });
        match kind {
            JobKind::MissingItemSearch | JobKind::RssSync => self.run_missing().await,
            JobKind::ManualSearch { content_id } => match self.one_node(content_id).await {
                Ok(Some(node)) => match self.run_node(&node).await {
                    Ok(_) => JobResult::Success,
                    Err(detail) => JobResult::Retryable { detail },
                },
                // A manual search for a content id that no longer exists is a
                // permanent (non-retryable) miss, not a flap.
                Ok(None) => JobResult::Permanent {
                    detail: format!("manual search: content {content_id} not found"),
                },
                Err(detail) => JobResult::Permanent { detail },
            },
            // Not pipeline work: a benign success so the scheduler keeps its cadence.
            JobKind::MetadataRefresh | JobKind::DiskSpaceCheck => JobResult::Success,
        }
    }
}

/// A stable label for the `CommandQueued` event, mirroring the API's command
/// names without depending on its private mapping.
fn command_label(kind: &JobKind) -> &'static str {
    match kind {
        JobKind::RssSync => "RssSync",
        JobKind::MissingItemSearch => "MissingItemSearch",
        JobKind::MetadataRefresh => "RefreshMetadata",
        JobKind::DiskSpaceCheck => "DiskSpaceCheck",
        JobKind::ManualSearch { .. } => "ManualSearch",
    }
}

// ---------------------------------------------------------------------------
// The daemon's live, DB-backed environment.
// ---------------------------------------------------------------------------

/// The daemon's [`PipelineEnv`]: resolves the live indexer set, the configured
/// download client, and a per-node [`RunnerConfig`] from the persistence layer.
///
/// Built once at boot and shared. Everything it returns is read from the DB *at
/// run time* so configuration CRUD (a new indexer, a reconfigured client, a new
/// remote-path mapping) takes effect on the next pipeline run with no restart.
/// The per-host rate limiter is shared across every run so indexers on one
/// tracker host share the budget the tracker enforces.
pub struct LivePipelineEnv {
    db: Database,
    rate_limiter: Arc<HostRateLimiter>,
    /// How many times Track polls the download client before giving up.
    max_track_polls: u32,
    /// How long to wait between Track polls so the poll budget spans a real,
    /// multi-minute download.
    track_poll_interval: std::time::Duration,
}

impl LivePipelineEnv {
    /// Build the live environment over the persistence handle.
    #[must_use]
    pub fn new(db: Database) -> Self {
        Self {
            db,
            rate_limiter: Arc::new(HostRateLimiter::conservative_default()),
            // A real download takes minutes; poll every few seconds for a budget
            // that spans it rather than burning instant polls.
            max_track_polls: 240,
            track_poll_interval: std::time::Duration::from_secs(5),
        }
    }

    /// Pick the enabled download client to grab through: the highest-priority
    /// (lowest `priority` number) enabled client, building its native adapter and
    /// returning it alongside the category its grabs are tagged with. `Ok(None)`
    /// when no enabled client is configured (a benign no-op — the daemon simply
    /// has nothing to grab with yet).
    async fn resolve_client(&self) -> Result<Option<(ConfiguredDownloadClient, String)>, String> {
        let mut clients = self
            .db
            .config()
            .list_download_clients()
            .await
            .map_err(|e| format!("loading download clients failed: {e}"))?;
        clients.retain(|c| c.enabled);
        clients.sort_by_key(|c| c.priority);
        let Some(config) = clients.first() else {
            return Ok(None);
        };
        let client = ConfiguredDownloadClient::from_config(config)
            .map_err(|e| format!("building download client '{}': {e}", config.name))?;
        Ok(Some((client, config.category.clone())))
    }

    /// The download category an interactive search tags its (hypothetical) grabs
    /// with, read **without building any client adapter**.
    ///
    /// The preview never grabs, so it never needs a live client — but the
    /// [`RunnerConfig`] still carries a `category`. We read it from the
    /// highest-priority enabled client's config when there is one, falling back to
    /// a sane default otherwise. Critically, this only *reads the config row* and
    /// never calls [`ConfiguredDownloadClient::from_config`], so a misconfigured or
    /// unreachable download client (e.g. a SABnzbd row missing its base URL) can
    /// never make an interactive search fail.
    async fn search_category(&self) -> Result<String, String> {
        let mut clients = self
            .db
            .config()
            .list_download_clients()
            .await
            .map_err(|e| format!("loading download clients failed: {e}"))?;
        clients.retain(|c| c.enabled);
        clients.sort_by_key(|c| c.priority);
        Ok(clients
            .first()
            .map(|c| c.category.clone())
            .unwrap_or_else(|| "cellarr".to_string()))
    }

    /// Resolve the seams an **interactive release search** needs: the live indexer
    /// set + the per-node [`RunnerConfig`], **without building a download client**.
    ///
    /// A search runs the read-only Discover→Decide preview, which never grabs, so
    /// it must never construct a download client (and so can never be failed by a
    /// misconfigured one). Discover itself is already per-source resilient (the
    /// `DbIndexerSet` skips a misconfigured/unreachable indexer with a warning),
    /// so a search returns the candidates it could gather (possibly empty) rather
    /// than erroring on any one bad source.
    ///
    /// Returns `Ok(None)` when the environment is not ready (no library root /
    /// quality profile for the node) — the shim renders that as a clearly-flagged
    /// empty result, not an error.
    async fn resolve_for_search(
        &self,
        content: &ContentRef,
    ) -> Result<Option<(DbIndexerSet, RunnerConfig)>, String> {
        // Read the category without constructing a client adapter (a preview never
        // grabs, so a misconfigured client must not block the search).
        let category = self.search_category().await?;
        let Some(config) = self.resolve_config(content, category).await? else {
            return Ok(None);
        };
        let indexer = DbIndexerSet::with_rate_limiter(
            self.db.clone(),
            Arc::clone(&self.rate_limiter),
            /* fail_fast = */ false,
        );
        Ok(Some((indexer, config)))
    }

    /// Build the [`RunnerConfig`] for a node: library root + naming + the library's
    /// default quality profile + custom formats + remote-path mappings, all from
    /// the DB. `client_category` is the category the chosen client tags grabs with
    /// (so cellarr only ever touches its own downloads).
    async fn resolve_config(
        &self,
        content: &ContentRef,
        client_category: String,
    ) -> Result<Option<RunnerConfig>, String> {
        let config_repo = self.db.config();

        // The node's library gives the root the import lands under and the default
        // quality profile to decide against.
        let library = config_repo
            .get_library(content.library_id)
            .await
            .map_err(|e| format!("loading library failed: {e}"))?;
        let Some(library) = library else {
            return Ok(None);
        };
        let Some(library_root) = library.root_folders.first().cloned() else {
            // A library with no root folder cannot import; surfaced as not-ready
            // rather than picking an arbitrary path.
            return Ok(None);
        };

        let profile = self
            .db
            .profiles()
            .get_profile(library.default_quality_profile)
            .await
            .map_err(|e| format!("loading quality profile failed: {e}"))?;
        let Some(profile) = profile else {
            return Ok(None);
        };
        let custom_formats = self
            .db
            .profiles()
            .custom_formats()
            .await
            .map_err(|e| format!("loading custom formats failed: {e}"))?;

        let remote_path_mappings = config_repo
            .list_remote_path_mappings()
            .await
            .map_err(|e| format!("loading remote-path mappings failed: {e}"))?;

        Ok(Some(RunnerConfig {
            profile,
            custom_formats,
            ranking: QualityRanking::default(),
            proper_repack_policy: ProperRepackPolicy::default(),
            library_root: std::path::PathBuf::from(library_root),
            naming_format: default_naming_format(content.media_type),
            // The aggregate indexer is type-erased; attribution ids identify the
            // configured set/client the grab is tagged to.
            indexer_id: cellarr_core::IndexerId::new(),
            client_id: cellarr_core::DownloadClientId::new(),
            category: client_category,
            max_track_polls: self.max_track_polls,
            track_poll_interval: self.track_poll_interval,
            // The client host scopes which remote-path mappings apply; cellarr runs
            // alongside the client in the default deployment (no rewrite needed),
            // so it is left empty and the mappings list is a no-op unless a mapping
            // names a host — which, when present, the runner applies.
            client_host: String::new(),
            remote_path_mappings,
        }))
    }
}

#[async_trait]
impl PipelineEnv for LivePipelineEnv {
    type Indexer = DbIndexerSet;
    type Client = ConfiguredDownloadClient;

    async fn resolve(
        &self,
        content: &ContentRef,
    ) -> Result<Option<(Self::Indexer, Self::Client, RunnerConfig)>, String> {
        let Some((client, client_category)) = self.resolve_client().await? else {
            return Ok(None);
        };
        let Some(config) = self.resolve_config(content, client_category).await? else {
            return Ok(None);
        };
        let indexer = DbIndexerSet::with_rate_limiter(
            self.db.clone(),
            Arc::clone(&self.rate_limiter),
            /* fail_fast = */ false,
        );
        Ok(Some((indexer, client, config)))
    }
}

// ---------------------------------------------------------------------------
// The interactive release-search seam (GET /api/v3/release).
// ---------------------------------------------------------------------------

/// The daemon's [`ReleaseSearch`](cellarr_api::release_search::ReleaseSearch)
/// implementation: drives the **read-only** Discover→Decide preview for one
/// content node through the real [`PipelineRunner`], returning ranked candidates
/// without grabbing.
///
/// It resolves the live indexer set + per-node [`RunnerConfig`] via a
/// [`LivePipelineEnv`] — but, unlike a pipeline run, it **never builds a download
/// client**: a search runs the read-only Discover→Decide preview and never grabs,
/// so the misconfigured/unreachable download client that would fail an
/// acquisition can never fail an interactive search. The score, quality, and
/// reject reason the screen shows still match exactly what a real acquisition
/// would compute. Discover is per-source resilient (a bad indexer is skipped with
/// a warning), so a search returns the candidates it could gather (possibly
/// empty) rather than erroring on one bad source. A node with no environment
/// ready (no library root / quality profile) is reported as
/// [`Unavailable`](cellarr_api::release_search::ReleaseSearchOutcome::Unavailable)
/// — a benign empty result, not an error.
///
/// No download client is constructed or driven here: the preview stops before
/// Grab, so no download is ever created by an interactive search.
pub struct LiveReleaseSearch {
    db: Database,
    registry: Arc<MediaRegistry>,
    env: LivePipelineEnv,
    clock: SystemClock,
}

impl LiveReleaseSearch {
    /// Build the interactive-search seam over the persistence handle + the shared
    /// media registry. It owns its own [`LivePipelineEnv`] so a search resolves
    /// indexers/config freshly per call (CRUD writes take effect with no restart).
    #[must_use]
    pub fn new(db: Database, registry: Arc<MediaRegistry>) -> Self {
        let env = LivePipelineEnv::new(db.clone());
        Self {
            db,
            registry,
            env,
            clock: SystemClock,
        }
    }
}

#[async_trait]
impl cellarr_api::release_search::ReleaseSearch for LiveReleaseSearch {
    async fn search(
        &self,
        content: cellarr_core::ContentId,
    ) -> Result<cellarr_api::release_search::ReleaseSearchOutcome, String> {
        use cellarr_api::release_search::ReleaseSearchOutcome;

        let node = self
            .db
            .content()
            .get(content)
            .await
            .map_err(|e| format!("loading content {content} failed: {e}"))?;
        let Some(node) = node else {
            return Err(format!("content {content} not found"));
        };

        // Resolve the live indexer + config for this node — WITHOUT building a
        // download client. A search never grabs, so a misconfigured/unreachable
        // download client must never fail it; the preview is driven over a no-op
        // client that is never called. `None` means the env is not ready (no
        // library root / quality profile) — an empty, clearly-flagged interactive
        // result rather than an error.
        let Some((indexer, config)) = self.env.resolve_for_search(&node).await? else {
            return Ok(ReleaseSearchOutcome::Unavailable(
                "no library root / quality profile configured for this node yet".into(),
            ));
        };

        // The preview stops before Grab, so the client is never driven; supply a
        // no-op rather than constructing a live adapter.
        let client = NoopDownloadClient;
        let runner = PipelineRunner::new(
            &indexer,
            &client,
            &self.registry,
            &self.db,
            &self.clock,
            &config,
        );
        let candidates = runner
            .preview_releases(&node)
            .await
            .map_err(|e| format!("interactive release search failed: {e}"))?;
        Ok(ReleaseSearchOutcome::Found(candidates))
    }
}

/// The default rename format per media type — the `{Token}` shape cellarr-fs
/// `render_name` interpolates against the media module's naming tokens. A
/// deployment can later make this configurable per library; this is the safe
/// built-in default the daemon uses today.
fn default_naming_format(media_type: MediaType) -> String {
    match media_type {
        MediaType::Tv => {
            "{Series Title}/Season {Season}/{Series Title} - S{Season}E{Episode}.{Extension}".into()
        }
        MediaType::Movie => "{Movie Title} ({Release Year})/{Movie Title}.{Extension}".into(),
        // Music/book libraries are not acquisition targets in v1; a flat,
        // extension-preserving default keeps any future node import safe.
        _ => "{Title}.{Extension}".into(),
    }
}
