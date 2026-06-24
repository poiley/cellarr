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
use cellarr_jobs::{
    DbIndexerSet, JobHandler, JobKind, JobResult, ProviderNotifier, WebhookNotifier,
};
use cellarr_media::MediaRegistry;

use crate::clients::{ConfiguredDownloadClient, NoopDownloadClient};

/// The app identity stamped onto the notifications the daemon fires (the Connect
/// webhook `instanceName` and the provider message instance name). cellarr has no
/// user-configurable instance name yet; this is the stable default.
const NOTIFICATION_INSTANCE_NAME: &str = "cellarr";

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
    /// The content-metadata resolver, attached to each run so a confident identify
    /// persists the node's facts + artwork, and driving `RefreshMetadata`. `None`
    /// (the default) means no metadata persistence (the offline path).
    resolver: Option<Arc<dyn cellarr_media::DynMetadataResolver>>,
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
            resolver: None,
        }
    }

    /// Attach a content-metadata resolver so identify persists metadata/artwork and
    /// `RefreshMetadata` does real work. Builder form so the offline path stays
    /// resolver-less.
    #[must_use]
    pub fn with_resolver(mut self, resolver: Arc<dyn cellarr_media::DynMetadataResolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// `RefreshMetadata`: re-resolve and persist the content-scoped metadata (and
    /// re-cache artwork) for every content node, via the resolver. With no resolver
    /// configured this is a no-op success (the offline path). A per-node failure is
    /// logged and the sweep continues — a refresh is best-effort enrichment.
    async fn refresh_metadata(&self) -> JobResult {
        let Some(resolver) = self.resolver.as_ref() else {
            return JobResult::Success;
        };
        let libraries = match self.db.config().list_libraries().await {
            Ok(l) => l,
            Err(detail) => {
                return JobResult::Retryable {
                    detail: format!("loading libraries failed: {detail}"),
                }
            }
        };
        let content = self.db.content();
        for lib in libraries {
            let mut stack = match content.roots(lib.id).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(library = %lib.id, error = %e, "refresh: loading roots failed");
                    continue;
                }
            };
            while let Some(node) = stack.pop() {
                let node_ref = node.as_ref();
                match resolver.resolve(&node_ref).await {
                    Ok(Some(resolved)) if !resolved.meta.is_empty() => {
                        if let Err(e) = content.set_metadata(node.id, &resolved.meta).await {
                            tracing::warn!(content = %node.id, error = %e, "refresh: persisting metadata failed");
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(content = %node.id, error = %e, "refresh: resolve failed");
                    }
                }
                match content.children(node.id).await {
                    Ok(children) => stack.extend(children),
                    Err(e) => {
                        tracing::warn!(content = %node.id, error = %e, "refresh: loading children failed");
                    }
                }
            }
        }
        JobResult::Success
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
        let mut runner = PipelineRunner::new(
            &indexer,
            &client,
            &self.registry,
            &self.db,
            &self.clock,
            &config,
        );
        if let Some(resolver) = self.resolver.as_ref() {
            runner = runner.with_metadata_resolver(resolver.clone());
        }
        // Attach the live notification dispatchers so Grab/Import/Upgrade
        // transitions fire the configured Connect webhook *and* the broadened
        // providers (Discord/Telegram/Email/Custom Script/media-server rescans).
        // Both are best-effort: a dead receiver is logged inside the dispatcher
        // and never affects the run, so wiring them in is always safe.
        runner = runner
            .with_notifier(WebhookNotifier::new(
                self.db.clone(),
                Arc::new(cellarr_api::ReqwestWebhookSender::new()),
                NOTIFICATION_INSTANCE_NAME,
            ))
            .with_provider_notifier(ProviderNotifier::new(
                self.db.clone(),
                cellarr_api::default_senders(),
                NOTIFICATION_INSTANCE_NAME,
            ));
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
        publish_outcome(&self.events, content, outcome);
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

/// Translate a terminal [`RunOutcome`] into the live [`DomainEvent`]s the UI/WS
/// subscribe to, onto `events`. Shared by the automatic pipeline handler and the
/// interactive grab seam so a manual grab surfaces the same progress an automatic
/// acquisition does. The runner already wrote the authoritative decision log +
/// history; this only mirrors the transition onto the push bus.
fn publish_outcome(events: &EventBus, content: &ContentRef, outcome: &RunOutcome) {
    match outcome {
        RunOutcome::Imported {
            grab_id,
            destinations,
        } => {
            events.publish(DomainEvent::QueueProgress {
                grab_id: grab_id.to_string(),
                status: "imported".to_string(),
                progress: Some(1.0),
            });
            for path in destinations {
                events.publish(DomainEvent::ImportCompleted {
                    content_id: content.id.to_string(),
                    path: path.clone(),
                });
            }
        }
        RunOutcome::Rejected { reason } => {
            events.publish(DomainEvent::DecisionLogged {
                run_id: content.id.to_string(),
                note: format!("rejected: {reason}"),
            });
        }
        RunOutcome::Failed { detail } => {
            events.publish(DomainEvent::DecisionLogged {
                run_id: content.id.to_string(),
                note: format!("grab failed: {detail}"),
            });
        }
        RunOutcome::HeldForReview { reason } => {
            events.publish(DomainEvent::DecisionLogged {
                run_id: content.id.to_string(),
                note: format!("held for review: {reason}"),
            });
        }
        RunOutcome::NothingFound => {
            events.publish(DomainEvent::DecisionLogged {
                run_id: content.id.to_string(),
                note: "no releases found".to_string(),
            });
        }
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
            // Re-resolve and persist content metadata + artwork for the library.
            JobKind::MetadataRefresh => self.refresh_metadata().await,
            // Not pipeline work: a benign success so the scheduler keeps its cadence.
            JobKind::DiskSpaceCheck => JobResult::Success,
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

        // Build the per-indexer criteria/priority lookup the decision engine
        // consults: minimum-seeders + required-flag (freeleech) gating and the
        // priority tie-break, keyed by the indexer id each release carries.
        let indexer_criteria = config_repo
            .list_indexers()
            .await
            .map_err(|e| format!("loading indexers failed: {e}"))?
            .into_iter()
            .map(|ix| (ix.id, (ix.criteria, ix.priority)))
            .collect();

        // Delay profiles hold a grabbable release for its protocol's window so a
        // better one can arrive first. Loaded so the runner resolves the governing
        // profile; an empty set (the default) imposes no delay.
        let delay_profiles = self
            .db
            .profiles()
            .list_delay_profiles()
            .await
            .map_err(|e| format!("loading delay profiles failed: {e}"))?;

        // The library-wide media-management settings drive the on-disk naming
        // format (per media type), the post-commit chmod/chown policy, and the
        // extra-file import policy. Absent settings resolve to defaults, preserving
        // prior behavior (built-in naming, no permission changes, no extras).
        let media_management = config_repo
            .get_media_management()
            .await
            .map_err(|e| format!("loading media-management settings failed: {e}"))?;
        let naming_format = media_management.naming.format_for(content.media_type);

        Ok(Some(RunnerConfig {
            profile,
            custom_formats,
            ranking: QualityRanking::default(),
            proper_repack_policy: ProperRepackPolicy::default(),
            library_root: std::path::PathBuf::from(library_root),
            naming_format,
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
            // Write Kodi/Jellyfin `.nfo` sidecars on import (media-management
            // default). Best-effort, post-commit, so it never affects crash safety.
            write_nfo: true,
            delay_profiles,
            // Content tags are not yet modeled on the node; the catch-all (tagless)
            // delay profile governs every node until per-node tags are wired.
            content_tags: Vec::new(),
            // Post-commit, best-effort policies from media-management settings.
            permissions: media_management.permissions.clone(),
            extra_files: media_management.extra_files.clone(),
            indexer_criteria,
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

// ---------------------------------------------------------------------------
// The interactive grab seam (POST /api/v3/release).
// ---------------------------------------------------------------------------

/// The daemon's [`ReleaseGrab`](cellarr_api::release_search::ReleaseGrab)
/// implementation: grabs the release a user picked from the interactive-search
/// screen and drives it through the real [`PipelineRunner`] Grab→Track→Import.
///
/// Unlike [`LiveReleaseSearch`] (which never builds a download client), a grab
/// **does** build the configured client — that is the whole point of the action.
/// It resolves the full live env via [`LivePipelineEnv::resolve`] (indexer +
/// download client + per-node config) and calls
/// [`PipelineRunner::grab_release`](cellarr_jobs::PipelineRunner::grab_release)
/// with the chosen guid. A node with no environment ready (no enabled download
/// client / library root / quality profile) is reported as
/// [`Unavailable`](cellarr_api::release_search::ReleaseGrabOutcome::Unavailable) —
/// a benign message, not an error. The matching domain events are published onto
/// the shared bus so the UI/WS observe the grab the same way an automatic one is
/// observed.
pub struct LiveReleaseGrab {
    db: Database,
    registry: Arc<MediaRegistry>,
    events: EventBus,
    env: LivePipelineEnv,
    clock: SystemClock,
}

impl LiveReleaseGrab {
    /// Build the interactive-grab seam over the persistence handle + shared media
    /// registry + the shared event bus. It owns its own [`LivePipelineEnv`] so a
    /// grab resolves the live client/config freshly per call (CRUD writes take
    /// effect with no restart).
    #[must_use]
    pub fn new(db: Database, registry: Arc<MediaRegistry>, events: EventBus) -> Self {
        let env = LivePipelineEnv::new(db.clone());
        Self {
            db,
            registry,
            events,
            env,
            clock: SystemClock,
        }
    }
}

#[async_trait]
impl cellarr_api::release_search::ReleaseGrab for LiveReleaseGrab {
    async fn grab(
        &self,
        content: cellarr_core::ContentId,
        guid: &str,
    ) -> Result<cellarr_api::release_search::ReleaseGrabOutcome, String> {
        use cellarr_api::release_search::ReleaseGrabOutcome;

        let node = self
            .db
            .content()
            .get(content)
            .await
            .map_err(|e| format!("loading content {content} failed: {e}"))?;
        let Some(node) = node else {
            return Err(format!("content {content} not found"));
        };

        // A grab builds the real download client (unlike a search). No enabled
        // client / library root / profile -> not ready: a benign message.
        let Some((indexer, client, config)) = self.env.resolve(&node).await? else {
            return Ok(ReleaseGrabOutcome::Unavailable(
                "no enabled download client / library root configured to grab with yet".into(),
            ));
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
            .grab_release(&node, guid)
            .await
            .map_err(|e| format!("interactive grab failed: {e}"))?;

        // Surface the same domain events an automatic run publishes, then map the
        // terminal outcome to the FE-facing grab result.
        publish_outcome(&self.events, &node, &outcome);
        let imported = matches!(outcome, RunOutcome::Imported { .. });
        let detail = match &outcome {
            RunOutcome::Imported { destinations, .. } => {
                format!("grabbed and imported ({} file(s))", destinations.len())
            }
            RunOutcome::Rejected { reason } => format!("not grabbed: {reason}"),
            RunOutcome::Failed { detail } => format!("grab failed: {detail}"),
            RunOutcome::HeldForReview { reason } => format!("held for review: {reason}"),
            RunOutcome::NothingFound => "release is no longer offered by the indexers".to_string(),
        };
        Ok(ReleaseGrabOutcome::Grabbed { imported, detail })
    }
}

// ---------------------------------------------------------------------------
// The manual-import seam (GET/POST /api/v3/manualimport).
// ---------------------------------------------------------------------------

/// The daemon's [`ManualImport`](cellarr_api::manual_import::ManualImport)
/// implementation: scans a loose folder (read-only) and commits the user's chosen
/// files through the real [`PipelineRunner`]'s crash-safe import path.
///
/// Neither the scan nor the commit grabs anything, so — like
/// [`LiveReleaseSearch`] — it never builds a download client: it drives the runner
/// over a [`NoopDownloadClient`] that is never called. The scan reuses the same
/// per-library [`RunnerConfig`] an acquisition uses (so files are parsed/identified
/// identically) and the commit reuses the same `plan_import`/`execute_import` path
/// (so a manual import is as crash-safe as an automatic one).
///
/// The config is resolved per call so library CRUD takes effect with no restart;
/// a folder scanned with no library configured at all is reported as
/// [`Unavailable`](cellarr_api::manual_import::ManualImportOutcome::Unavailable) —
/// a benign empty result, not an error.
pub struct LiveManualImport {
    db: Database,
    registry: Arc<MediaRegistry>,
    env: LivePipelineEnv,
    clock: SystemClock,
}

impl LiveManualImport {
    /// Build the manual-import seam over the persistence handle + the shared media
    /// registry. It owns its own [`LivePipelineEnv`] so it resolves library config
    /// freshly per call.
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

    /// Resolve a [`RunnerConfig`] for the manual-import runner, scoped to the first
    /// configured library of any media type (the scan/commit naming + library root
    /// come from it). A loose folder is not tied to a single node, so we pick the
    /// first library with a usable root + profile. `Ok(None)` when none is ready.
    async fn resolve_config(&self) -> Result<Option<RunnerConfig>, String> {
        let libraries = self
            .db
            .config()
            .list_libraries()
            .await
            .map_err(|e| format!("loading libraries failed: {e}"))?;
        // Build a config against the first library that has a root folder; the
        // commit renames each file using the *node's* media-type naming format, so
        // the library chosen here only supplies the root + profile/ranking inputs.
        for library in libraries {
            // A synthetic node ref scoped to this library, of the library's media
            // type, lets the existing per-node config resolver build the config. The
            // ref is used only to read the library's root/profile, never persisted.
            let probe = ContentRef::new(
                cellarr_core::ContentId::new(),
                library.id,
                library.media_type,
                default_coords(library.media_type),
            )
            .map_err(|e| format!("building probe ref failed: {e}"))?;
            if let Some(config) = self
                .env
                .resolve_config(&probe, "cellarr".to_string())
                .await?
            {
                return Ok(Some(config));
            }
        }
        Ok(None)
    }
}

#[async_trait]
impl cellarr_api::manual_import::ManualImport for LiveManualImport {
    async fn scan(
        &self,
        folder: &str,
    ) -> Result<cellarr_api::manual_import::ManualImportOutcome, String> {
        use cellarr_api::manual_import::ManualImportOutcome;

        let Some(config) = self.resolve_config().await? else {
            return Ok(ManualImportOutcome::Unavailable(
                "no library with a root folder is configured yet".into(),
            ));
        };
        // Scan + identify never grab, so the client is never driven.
        let client = NoopDownloadClient;
        let indexer = DbIndexerSet::new(self.db.clone());
        let runner = PipelineRunner::new(
            &indexer,
            &client,
            &self.registry,
            &self.db,
            &self.clock,
            &config,
        );
        let candidates = runner
            .scan_manual_import(std::path::Path::new(folder))
            .await
            .map_err(|e| format!("manual-import scan failed: {e}"))?;
        Ok(ManualImportOutcome::Found(candidates))
    }

    async fn commit(
        &self,
        items: Vec<cellarr_api::manual_import::ManualImportRequest>,
    ) -> Result<cellarr_api::manual_import::ManualImportCommitOutcome, String> {
        use cellarr_api::manual_import::ManualImportCommitOutcome;

        let Some(config) = self.resolve_config().await? else {
            return Ok(ManualImportCommitOutcome::Unavailable(
                "no library with a root folder is configured yet".into(),
            ));
        };
        let client = NoopDownloadClient;
        let indexer = DbIndexerSet::new(self.db.clone());
        let runner = PipelineRunner::new(
            &indexer,
            &client,
            &self.registry,
            &self.db,
            &self.clock,
            &config,
        );
        let (imported, errors) = runner
            .import_manual(&items)
            .await
            .map_err(|e| format!("manual import failed: {e}"))?;
        Ok(ManualImportCommitOutcome::Committed { imported, errors })
    }
}

/// The default coordinates for a probe content ref of `media_type` (used only to
/// build a library-scoped [`RunnerConfig`]; never persisted).
fn default_coords(media_type: MediaType) -> cellarr_core::Coordinates {
    match media_type {
        MediaType::Tv => cellarr_core::Coordinates::Episode {
            season: 1,
            episode: 1,
            absolute: None,
        },
        MediaType::Movie => cellarr_core::Coordinates::Movie,
        MediaType::Music => cellarr_core::Coordinates::Track { disc: 1, track: 1 },
        MediaType::Book => cellarr_core::Coordinates::Book {
            series_position: None,
        },
    }
}
