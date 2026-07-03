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
use cellarr_core::{ContentRef, DownloadClient, Indexer, MediaType};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_indexers::HostRateLimiter;
use cellarr_jobs::clock::SystemClock;
use cellarr_jobs::runner::{PipelineRunner, RunOutcome, RunnerConfig};
use cellarr_jobs::{
    DbIndexerSet, ImportListSync, JobHandler, JobKind, JobResult, ProviderNotifier, WebhookNotifier,
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
    /// The anime scene-mapping provider (TheTVDB/TheXEM), attached to each run so
    /// the Identify stage remaps an absolute episode number to its season/episode
    /// for an anime-typed series. `None` (the default) leaves the remap dead — an
    /// absolute release is then surfaced for manual resolution rather than guessed,
    /// so the absence is safe. The daemon constructs one from the meta-service and
    /// opts in via [`with_scene_provider`](Self::with_scene_provider).
    scene_provider: Option<Arc<dyn cellarr_media::DynSceneMappingProvider>>,
    /// The metadata **lookup** seam (title → candidates), attached for the opt-in
    /// auto-onboard: a rescan that cannot place a file looks its title up here.
    /// `None` disables auto-onboard regardless of the flag.
    metadata: Option<Arc<dyn cellarr_api::MetadataLookup>>,
    /// Whether the rescan auto-onboards (creates content from a confident metadata
    /// match for an otherwise-unplaceable file). Off by default.
    auto_onboard: bool,
    /// Cap on nodes created per onboard pass (`None` = unbounded) — for a staged
    /// first batch of a large library.
    auto_onboard_limit: Option<usize>,
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
            scene_provider: None,
            metadata: None,
            auto_onboard: false,
            auto_onboard_limit: None,
        }
    }

    /// Attach the metadata-lookup seam and enable the opt-in auto-onboard: a rescan
    /// that cannot place a file looks its parsed title up and, on a high-confidence
    /// match, creates the movie/series and adopts the file. `enabled = false` (or no
    /// seam) leaves the conservative adopt-to-existing-nodes behavior.
    #[must_use]
    pub fn with_auto_onboard(
        mut self,
        metadata: Arc<dyn cellarr_api::MetadataLookup>,
        enabled: bool,
        limit: Option<usize>,
    ) -> Self {
        self.metadata = Some(metadata);
        self.auto_onboard = enabled;
        self.auto_onboard_limit = limit;
        self
    }

    /// Attach the anime scene-mapping provider (TheTVDB/TheXEM) so the Identify
    /// stage's absolute→season/episode remap runs in the daemon for anime-typed
    /// series — the dead-in-prod fix. Builder form so the offline path stays
    /// provider-less (an absolute release is then surfaced for manual resolution,
    /// never guessed).
    #[must_use]
    pub fn with_scene_provider(
        mut self,
        provider: Arc<dyn cellarr_media::DynSceneMappingProvider>,
    ) -> Self {
        self.scene_provider = Some(provider);
        self
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
        // Attach the anime scene-mapping provider so the absolute→episode remap
        // runs in the daemon for anime-typed series (the dead-in-prod fix). With
        // none configured the runner surfaces an absolute release for manual
        // resolution rather than guessing — the offline-safe default.
        if let Some(scene_provider) = self.scene_provider.as_ref() {
            runner = runner.with_scene_provider(scene_provider.clone());
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

    /// `RescanLibrary`: reconcile on-disk files against the `media_file` table,
    /// one library at a time. Each library gets its own runner (built from that
    /// library's config) so a scanned file's destination is rendered under the
    /// correct root; the runner adopts the confident matches in place and leaves the
    /// rest for the manual-import screen. A per-library failure is logged and the
    /// sweep continues — one unreadable root must not strand the others.
    async fn run_rescan(&self) -> JobResult {
        let libraries = match self.db.config().list_libraries().await {
            Ok(l) => l,
            Err(detail) => {
                return JobResult::Retryable {
                    detail: format!("loading libraries failed: {detail}"),
                }
            }
        };
        // Roots that currently exist on disk — the ONLY roots the prune pass is
        // allowed to remove rows under, so a transient mount outage (every file
        // "missing") never mass-deletes the library and triggers a re-download.
        let mut accessible_roots: Vec<std::path::PathBuf> = Vec::new();
        for library in &libraries {
            for root in &library.root_folders {
                if tokio::fs::try_exists(root).await.unwrap_or(false) {
                    accessible_roots.push(std::path::PathBuf::from(root));
                }
            }
        }

        let (mut adopted, mut unmatched, mut errors, mut pruned, mut onboarded) =
            (0usize, 0usize, 0usize, 0usize, 0usize);
        let mut pruned_once = false;
        for library in libraries {
            // A synthetic node ref scoped to this library lets the env build the
            // library's config (root + naming); it is used only to resolve config,
            // never persisted. The indexer/client are unused by a rescan (it scans
            // and imports, never searches or grabs).
            let probe = match ContentRef::new(
                cellarr_core::ContentId::new(),
                library.id,
                library.media_type,
                default_coords(library.media_type),
            ) {
                Ok(p) => p,
                Err(detail) => {
                    tracing::warn!(library = %library.id, error = %detail, "rescan: probe build failed; skipping");
                    continue;
                }
            };
            let resolved = match self.env.resolve(&probe).await {
                Ok(r) => r,
                Err(detail) => {
                    tracing::warn!(library = %library.id, error = %detail, "rescan: env resolve failed; skipping");
                    continue;
                }
            };
            let Some((indexer, client, config)) = resolved else {
                // No environment ready for this library (no root/profile) — nothing
                // to reconcile here yet.
                continue;
            };
            let runner = PipelineRunner::new(
                &indexer,
                &client,
                &self.registry,
                &self.db,
                &self.clock,
                &config,
            );
            match runner.rescan().await {
                Ok(report) => {
                    adopted += report.adopted;
                    unmatched += report.unmatched;
                    errors += report.errors.len();
                }
                Err(detail) => {
                    tracing::warn!(library = %library.id, error = %detail, "rescan: library failed; continuing");
                }
            }
            // Opt-in AUTO-ONBOARD: after the adopt pass, the files still untracked
            // under this root are the ones no existing node could place. For each,
            // look its parsed title up in the metadata source and, on a HIGH-
            // confidence match (exact normalized title + year), create the
            // movie/series and adopt the file onto it. Off unless configured; a
            // no-confidence / ambiguous file is left for the manual-import screen —
            // content is never created from a guess.
            if self.auto_onboard {
                if let Some(meta) = self.metadata.clone() {
                    // A staged first batch caps how many nodes are created; the scan
                    // is bounded to match so a huge library is not fully walked just
                    // to onboard a few. The cap is a running total across libraries.
                    let remaining = self.auto_onboard_limit.map(|n| n.saturating_sub(onboarded));
                    if remaining == Some(0) {
                        // Cap already reached in an earlier library this pass.
                    } else {
                        match runner
                            .scan_manual_import(Some(&config.library_root), remaining)
                            .await
                        {
                        Ok(candidates) => {
                            for c in candidates {
                                if self.auto_onboard_limit.is_some_and(|n| onboarded >= n) {
                                    break;
                                }
                                if c.suggested.is_some() {
                                    continue; // an existing node fits — the adopt pass handled it.
                                }
                                let parsed = cellarr_parse::parse_title(&c.name);
                                let Some(title) = parsed.clean_title.clone() else {
                                    continue;
                                };
                                let results = match meta.search(library.media_type, &title).await {
                                    Ok(cellarr_api::LookupOutcome::Resolved(r)) => r,
                                    // Unavailable / errored source → skip; try next run.
                                    _ => continue,
                                };
                                let Some(chosen) =
                                    pick_confident_candidate(&results, &title, parsed.year)
                                else {
                                    continue; // no unambiguous match — leave for manual import.
                                };
                                let node_id = match self.create_content_node(&library, chosen).await
                                {
                                    Ok(id) => id,
                                    Err(detail) => {
                                        tracing::warn!(title = %title, error = %detail, "auto-onboard: create failed");
                                        continue;
                                    }
                                };
                                match runner
                                    .import_manual(&[cellarr_jobs::runner::ManualImportRequest {
                                        path: c.path.clone(),
                                        content_id: node_id,
                                    }])
                                    .await
                                {
                                    Ok((_imported, errs)) if errs.is_empty() => onboarded += 1,
                                    Ok((_imported, errs)) => {
                                        tracing::warn!(path = %c.path, ?errs, "auto-onboard: adopt failed after create");
                                    }
                                    Err(detail) => {
                                        tracing::warn!(path = %c.path, error = %detail, "auto-onboard: adopt errored");
                                    }
                                }
                            }
                        }
                        Err(detail) => {
                            tracing::warn!(library = %library.id, error = %detail, "auto-onboard: scan failed");
                        }
                        }
                    }
                }
            }
            // Prune vanished files once — it is global (all roots), so any one
            // runner does it; the media_file table is shared across libraries.
            if !pruned_once {
                pruned_once = true;
                match runner.prune_missing(&accessible_roots).await {
                    Ok(n) => pruned += n,
                    Err(detail) => {
                        tracing::warn!(error = %detail, "rescan: prune pass failed; continuing");
                    }
                }
            }
        }
        tracing::info!(adopted, unmatched, errors, pruned, onboarded, "library rescan complete");
        JobResult::Success
    }

    /// Create a content node from a confident metadata candidate, returning its id.
    /// Persists identity (title + external id) and metadata, then best-effort
    /// enriches via the resolver if one is attached (else the daily RefreshMetadata
    /// cron fills it in). The node is monitored so acquisition maintains it.
    async fn create_content_node(
        &self,
        library: &cellarr_core::Library,
        chosen: &cellarr_api::LookupCandidate,
    ) -> Result<cellarr_core::ContentId, String> {
        use cellarr_core::{ContentId, ContentKind, ContentNode, Coordinates};

        let (kind, coords) = match library.media_type {
            MediaType::Tv => (
                ContentKind::Series,
                Coordinates::Episode {
                    season: 1,
                    episode: 1,
                    absolute: None,
                },
            ),
            _ => (ContentKind::Movie, Coordinates::Movie),
        };
        let node = ContentNode {
            id: ContentId::new(),
            library_id: library.id,
            media_type: library.media_type,
            parent_id: None,
            kind,
            series_type: cellarr_core::SeriesType::default(),
            coords,
            monitored: true,
            title_id: None,
            tags: Vec::new(),
        };
        let content = self.db.content();
        content.upsert(&node).await.map_err(|e| e.to_string())?;
        content
            .index_title(node.id, &chosen.title)
            .await
            .map_err(|e| e.to_string())?;
        // Link the native external id so the resolver can enrich and search builds a
        // real query. Prefer the namespace the media type keys on.
        if let Some((scheme, value)) = native_external_id(chosen, library.media_type) {
            content
                .link_external_id(node.id, library.media_type, scheme, value, &chosen.title)
                .await
                .map_err(|e| e.to_string())?;
        }
        let meta = cellarr_core::ContentMetadata {
            title: Some(chosen.title.clone()),
            year: chosen.year,
            ..Default::default()
        };
        content
            .set_metadata(node.id, &meta)
            .await
            .map_err(|e| e.to_string())?;
        // Best-effort rich enrichment now (poster/overview/runtime); a failure is
        // fine — the daily metadata refresh retries.
        if let Some(resolver) = &self.resolver {
            if let Ok(node_ref) = ContentRef::new(
                node.id,
                library.id,
                library.media_type,
                default_coords(library.media_type),
            ) {
                let _ = resolver.resolve(&node_ref).await;
            }
        }
        Ok(node.id)
    }
}

/// The native external id `(scheme, value)` of a lookup candidate — tvdb for TV,
/// tmdb for movies, then imdb. `None` when the candidate carries no id.
fn native_external_id<'a>(
    candidate: &'a cellarr_api::LookupCandidate,
    media_type: MediaType,
) -> Option<(&'static str, &'a str)> {
    if media_type == MediaType::Tv {
        if let Some(v) = candidate.external_id("tvdb") {
            return Some(("tvdb", v));
        }
    }
    if let Some(v) = candidate.external_id("tmdb") {
        return Some(("tmdb", v));
    }
    candidate.external_id("imdb").map(|v| ("imdb", v))
}

/// Pick the single confident match for auto-onboard, or `None` to leave the file
/// for manual import. Confident = exactly one candidate whose normalized title
/// equals the parsed title and whose year lines up. Any ambiguity yields `None` —
/// content is never created from a guess.
///
/// Year handling prefers an EXACT match and only falls back to within-1: a file's
/// year is often the festival/premiere year while the source lists the release
/// year (300: 2006 vs 2007), so ±1 recovers the common off-by-one — but doing it
/// exact-first means a genuine exact-year hit still wins over an adjacent-year one
/// (so a duplicate a year apart doesn't spuriously make a clean match ambiguous).
fn pick_confident_candidate<'a>(
    results: &'a [cellarr_api::LookupCandidate],
    parsed_title: &str,
    parsed_year: Option<u16>,
) -> Option<&'a cellarr_api::LookupCandidate> {
    fn normalize(s: &str) -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    }
    let want = normalize(parsed_title);
    let title_matches: Vec<&cellarr_api::LookupCandidate> =
        results.iter().filter(|c| normalize(&c.title) == want).collect();

    // No parsed year → a single title match is confident.
    let Some(py) = parsed_year else {
        return (title_matches.len() == 1).then(|| title_matches[0]);
    };

    // Exact-year candidates; fall back to within-1 only if there is no exact year.
    let exact: Vec<&cellarr_api::LookupCandidate> = title_matches
        .iter()
        .copied()
        .filter(|c| c.year == Some(py))
        .collect();
    let pool = if exact.is_empty() {
        title_matches
            .iter()
            .copied()
            .filter(|c| c.year.is_some_and(|cy| py.abs_diff(cy) == 1))
            .collect()
    } else {
        exact
    };
    (pool.len() == 1).then(|| pool[0])
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
            // Reconcile on-disk files against the DB: adopt what parses, surface the rest.
            JobKind::RescanLibrary => self.run_rescan().await,
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
        JobKind::RescanLibrary => "RescanLibrary",
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
    /// Startup grace polls before stall detection (magnet metadata/DHT bootstrap).
    stall_grace_polls: u32,
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
            // A real multi-GB download takes far longer than a few minutes —
            // especially through a VPN. 240 polls (20 min) was "tracking timed out"
            // and blocklisting live downloads mid-flight; this is a backstop that
            // spans a very slow download (~24h at 5s). A genuinely dead torrent is
            // caught much sooner by the stall detector (stuck at ~0% with no peers),
            // not by this cap.
            max_track_polls: 17_280,
            track_poll_interval: std::time::Duration::from_secs(5),
            // ~60s grace for a magnet to fetch metadata + bootstrap peers before the
            // stall detector can fire (verified live: peers connect only after ~40s).
            stall_grace_polls: 12,
        }
    }

    /// The tag ids and (case-insensitive) labels on a content node, read from the
    /// persisted `content_tag` association and resolved against the tag
    /// vocabulary. The ids drive tag-scoped indexer/client/notification
    /// restrictions; the labels drive the (label-keyed) delay-profile resolution.
    /// An untagged node yields two empty vectors — the global path.
    async fn content_tags(&self, content: &ContentRef) -> Result<(Vec<u32>, Vec<String>), String> {
        let ids = self
            .db
            .content()
            .get_tags(content.id)
            .await
            .map_err(|e| format!("loading content tags failed: {e}"))?;
        let labels = self
            .db
            .tags()
            .labels_for(&ids)
            .await
            .map_err(|e| format!("resolving content tag labels failed: {e}"))?;
        Ok((ids, labels))
    }

    /// Pick the enabled download client to grab through: the highest-priority
    /// (lowest `priority` number) enabled client **that applies to the content's
    /// tags**, building its native adapter and returning it alongside the category
    /// its grabs are tagged with. A tag-scoped client is eligible only when it
    /// shares a tag id with `content_tags`; an untagged client is global.
    /// `Ok(None)` when no eligible enabled client is configured (a benign no-op).
    async fn resolve_client(
        &self,
        content_tags: &[u32],
    ) -> Result<Option<(ConfiguredDownloadClient, String)>, String> {
        let mut clients = self
            .db
            .config()
            .list_download_clients()
            .await
            .map_err(|e| format!("loading download clients failed: {e}"))?;
        clients.retain(|c| c.enabled && cellarr_core::tag_scope_applies(&c.tags, content_tags));
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
        let (content_tag_ids, _) = self.content_tags(content).await?;
        let indexer = DbIndexerSet::with_rate_limiter(
            self.db.clone(),
            Arc::clone(&self.rate_limiter),
            /* fail_fast = */ false,
        )
        .with_content_tags(content_tag_ids);
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

        // Release profiles gate and score a candidate by its required / ignored /
        // preferred terms (an ignored term or unmet required-term profile rejects;
        // matching preferred terms add to the score). Loaded so the decision engine
        // applies every enabled one whose tags match the node; an empty set (the
        // default) gates and scores nothing.
        let release_profiles = self
            .db
            .profiles()
            .list_release_profiles()
            .await
            .map_err(|e| format!("loading release profiles failed: {e}"))?;

        // The library-wide media-management settings drive the on-disk naming
        // format (per media type), the post-commit chmod/chown policy, and the
        // extra-file import policy. Absent settings resolve to defaults, preserving
        // prior behavior (built-in naming, no permission changes, no extras).
        let media_management = config_repo
            .get_media_management()
            .await
            .map_err(|e| format!("loading media-management settings failed: {e}"))?;
        let naming_format = media_management.naming.format_for(content.media_type);
        // The node's series type (resolved from the series root) selects the anime
        // episode naming format for an anime episode whose absolute number is known.
        // A non-TV node, or a read failure, resolves to the safe `Standard` default
        // so the standard format is used — preserving prior behavior. The composed
        // anime episode format is carried alongside so the runner can pick it
        // per-node; it is empty for a non-TV node (the standard format always wins).
        let series_type = if content.media_type == MediaType::Tv {
            self.db
                .content()
                .series_type_for(content.id)
                .await
                .unwrap_or_default()
        } else {
            cellarr_core::SeriesType::Standard
        };
        let anime_naming_format = if content.media_type == MediaType::Tv {
            media_management.naming.anime_episode_format()
        } else {
            String::new()
        };

        // The node's real tags drive tag-scoped routing: the labels feed the
        // label-keyed delay-profile resolution; the ids feed the id-keyed
        // indexer / download-client / notification restrictions. An untagged node
        // yields empties — the catch-all delay profile and every global config
        // apply, preserving prior behavior.
        let (content_tag_ids, content_tags) = self.content_tags(content).await?;

        // The quality catalogue with any persisted per-quality edits (titles +
        // size bounds) merged in, so the decision engine enforces the operator's
        // edited size bounds. Falls back to the code default on a read failure.
        let ranking = self
            .db
            .profiles()
            .quality_ranking()
            .await
            .map_err(|e| format!("loading quality definitions failed: {e}"))?;

        Ok(Some(RunnerConfig {
            profile,
            custom_formats,
            ranking,
            proper_repack_policy: ProperRepackPolicy::default(),
            library_root: std::path::PathBuf::from(library_root),
            naming_format,
            anime_naming_format,
            series_type,
            // The aggregate indexer is type-erased; attribution ids identify the
            // configured set/client the grab is tagged to.
            indexer_id: cellarr_core::IndexerId::new(),
            client_id: cellarr_core::DownloadClientId::new(),
            category: client_category,
            max_track_polls: self.max_track_polls,
            track_poll_interval: self.track_poll_interval,
            stall_grace_polls: self.stall_grace_polls,
            // The client host scopes which remote-path mappings apply; cellarr runs
            // alongside the client in the default deployment (no rewrite needed),
            // so it is left empty and the mappings list is a no-op unless a mapping
            // names a host — which, when present, the runner applies.
            client_host: String::new(),
            remote_path_mappings,
            // Write Kodi/Jellyfin `.nfo` sidecars on import when the metadata
            // consumer is enabled (the media-management `write_nfo` setting,
            // default on). Best-effort, post-commit, so it never affects crash
            // safety; the v3 `metadata` resource toggles this flag.
            write_nfo: media_management.write_nfo,
            delay_profiles,
            release_profiles,
            // The node's real tags: labels for the label-keyed delay profiles,
            // ids for the id-keyed indexer/client/notification restrictions.
            content_tags,
            content_tag_ids,
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
        // The node's tag ids scope which download client / indexers apply.
        let (content_tag_ids, _) = self.content_tags(content).await?;
        let Some((client, client_category)) = self.resolve_client(&content_tag_ids).await? else {
            return Ok(None);
        };
        let Some(config) = self.resolve_config(content, client_category).await? else {
            return Ok(None);
        };
        let indexer = DbIndexerSet::with_rate_limiter(
            self.db.clone(),
            Arc::clone(&self.rate_limiter),
            /* fail_fast = */ false,
        )
        .with_content_tags(content_tag_ids);
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
/// A small, shared cache of the exact [`Release`](cellarr_core::Release) objects
/// the interactive search last surfaced, keyed by the guid / download-url the UI
/// carries in each row.
///
/// The grab reads it so it can acquire the release the user *actually picked* —
/// the same bytes the search returned — WITHOUT re-querying the indexer. That
/// removes a redundant round-trip and, more importantly, the "no longer offered
/// by the indexers" race: an indexer's live listing (RSS/torrent search) shifts
/// minute to minute, so a re-Discover between the search and the grab can miss a
/// release the user is looking right at. Bounded so a long-lived daemon can't
/// grow it without limit (a fresh search for a node overwrites its rows).
#[derive(Default)]
pub struct GrabCandidateCache {
    inner: std::sync::Mutex<std::collections::HashMap<String, cellarr_core::Release>>,
}

/// The cache is a convenience, not a store of record — cap it so a very long
/// session can't accumulate unbounded entries; on overflow the whole map is
/// dropped (the next search repopulates the rows that matter).
const GRAB_CACHE_MAX: usize = 4096;

impl GrabCandidateCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Remember the candidates a search produced, keyed by both the indexer guid
    /// (when advertised) and the download url — the two ids a grab can arrive with.
    fn remember(&self, candidates: &[cellarr_jobs::ReleaseCandidate]) {
        let Ok(mut map) = self.inner.lock() else {
            return; // a poisoned lock only costs the cache; the grab re-discovers.
        };
        if map.len() >= GRAB_CACHE_MAX {
            map.clear();
        }
        for c in candidates {
            if let Some(guid) = c.release.guid.as_deref() {
                map.insert(guid.to_string(), c.release.clone());
            }
            map.insert(c.release.download_url.clone(), c.release.clone());
        }
    }

    /// The cached release for a guid / download-url the UI grabbed, if the search
    /// that produced it is still remembered. `None` falls back to a re-Discover.
    fn get(&self, key: &str) -> Option<cellarr_core::Release> {
        self.inner.lock().ok()?.get(key).cloned()
    }
}

/// No download client is constructed or driven here: the preview stops before
/// Grab, so no download is ever created by an interactive search.
pub struct LiveReleaseSearch {
    db: Database,
    registry: Arc<MediaRegistry>,
    env: LivePipelineEnv,
    clock: SystemClock,
    /// The anime scene-mapping provider, attached to the preview runner so the
    /// interactive search remaps an absolute release the same way an acquisition
    /// would (the score/placement the user previews matches a real run). `None`
    /// leaves an absolute release un-remapped (skipped from the preview), never
    /// guessed.
    scene_provider: Option<Arc<dyn cellarr_media::DynSceneMappingProvider>>,
    /// The candidate cache this search populates; the grab seam reads from the
    /// same instance so it can acquire the exact release the user picked without a
    /// re-Discover. Its own private cache by default (harmless when unshared).
    cache: Arc<GrabCandidateCache>,
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
            scene_provider: None,
            cache: Arc::new(GrabCandidateCache::new()),
        }
    }

    /// Attach the anime scene-mapping provider so the interactive search remaps an
    /// absolute release for anime-typed series (parity with an acquisition).
    #[must_use]
    pub fn with_scene_provider(
        mut self,
        provider: Arc<dyn cellarr_media::DynSceneMappingProvider>,
    ) -> Self {
        self.scene_provider = Some(provider);
        self
    }

    /// Share the candidate cache with the grab seam so a grab can acquire the exact
    /// release this search surfaced without re-querying the indexer.
    #[must_use]
    pub fn with_grab_cache(mut self, cache: Arc<GrabCandidateCache>) -> Self {
        self.cache = cache;
        self
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
        let mut runner = PipelineRunner::new(
            &indexer,
            &client,
            &self.registry,
            &self.db,
            &self.clock,
            &config,
        );
        if let Some(scene_provider) = self.scene_provider.as_ref() {
            runner = runner.with_scene_provider(scene_provider.clone());
        }
        let candidates = runner
            .preview_releases(&node)
            .await
            .map_err(|e| format!("interactive release search failed: {e}"))?;
        // Remember the exact releases so a subsequent grab of any of these rows can
        // acquire it directly, without re-querying the indexer.
        self.cache.remember(&candidates);
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
    /// The anime scene-mapping provider, attached to the grab runner so a manual
    /// grab of an absolute release for an anime-typed series remaps to the correct
    /// season/episode (parity with an automatic acquisition). `None` surfaces an
    /// absolute release for manual resolution rather than guessing.
    scene_provider: Option<Arc<dyn cellarr_media::DynSceneMappingProvider>>,
    /// Shared with the search seam: the exact releases the last search surfaced, so
    /// a grab acquires the user's pick without re-querying the indexer.
    cache: Arc<GrabCandidateCache>,
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
            scene_provider: None,
            cache: Arc::new(GrabCandidateCache::new()),
        }
    }

    /// Attach the anime scene-mapping provider so a manual grab remaps an absolute
    /// release for anime-typed series (parity with an automatic acquisition).
    #[must_use]
    pub fn with_scene_provider(
        mut self,
        provider: Arc<dyn cellarr_media::DynSceneMappingProvider>,
    ) -> Self {
        self.scene_provider = Some(provider);
        self
    }

    /// Share the candidate cache the interactive search populates, so a grab can
    /// acquire the exact release the user picked without re-querying the indexer.
    #[must_use]
    pub fn with_grab_cache(mut self, cache: Arc<GrabCandidateCache>) -> Self {
        self.cache = cache;
        self
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
        // client / library root / profile -> not ready: a benign message. Resolved
        // synchronously so a misconfigured environment is reported to the user
        // immediately (before the request returns), not swallowed in the background.
        let Some((indexer, client, config)) = self.env.resolve(&node).await? else {
            return Ok(ReleaseGrabOutcome::Unavailable(
                "no enabled download client / library root configured to grab with yet".into(),
            ));
        };

        // Prefer the exact release the search surfaced (no indexer re-query, no
        // "no longer offered" race); fall back to a guid re-Discover when the cache
        // has aged out.
        let cached = self.cache.get(guid);

        // Drive Grab→Track→Import on a background task and return immediately. The
        // download runs for minutes (Track polls the client until the file lands),
        // so blocking the HTTP request on it is what made the row hang forever. The
        // spawned run publishes the same domain events an automatic acquisition does
        // — Grabbed, then Imported/Rejected/etc — which the UI already observes over
        // the WebSocket and the /queue view, so the user sees real progress.
        let db = self.db.clone();
        let registry = Arc::clone(&self.registry);
        let events = self.events.clone();
        let clock = self.clock;
        let scene = self.scene_provider.clone();
        let guid_owned = guid.to_string();
        tokio::spawn(async move {
            let mut runner =
                PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
            if let Some(scene_provider) = scene.as_ref() {
                runner = runner.with_scene_provider(scene_provider.clone());
            }
            let outcome = match &cached {
                Some(release) => runner.grab_selected_release(&node, release).await,
                None => runner.grab_release(&node, &guid_owned).await,
            };
            match outcome {
                // The runner already logs each stage transition; publish the same
                // domain events an automatic run does so the UI reflects the result.
                Ok(o) => publish_outcome(&events, &node, &o),
                Err(e) => {
                    tracing::warn!(guid = %guid_owned, error = %e, "interactive grab failed");
                    events.publish(DomainEvent::DecisionLogged {
                        run_id: node.id.to_string(),
                        note: format!("interactive grab failed: {e}"),
                    });
                }
            }
        });

        // The grab was accepted and is now downloading in the background.
        Ok(ReleaseGrabOutcome::Grabbed {
            imported: false,
            detail: "queued for download — cellarr is tracking it in the background".into(),
        })
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
        folder: Option<&str>,
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
        // `None` (no folder) scans the library roots so untracked in-place files
        // auto-surface; `Some(folder)` scans that loose folder. Interactive scans
        // cap the candidate count: a barely-tracked library has tens of thousands of
        // untracked files, and the review screen only needs a bounded preview (the
        // background rescan job reconciles the rest, unbounded).
        const MANUAL_IMPORT_SCAN_CAP: usize = 500;
        let candidates = runner
            .scan_manual_import(folder.map(std::path::Path::new), Some(MANUAL_IMPORT_SCAN_CAP))
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

// ---------------------------------------------------------------------------
// The import-list sync seam (POST /api/v3/importlist/{id}/sync + ImportListSync).
// ---------------------------------------------------------------------------

/// The daemon's
/// [`ImportListSyncRunner`](cellarr_api::import_list_sync::ImportListSyncRunner)
/// implementation: runs the safeguarded fetch+add for the configured import lists
/// through the real [`cellarr_jobs::ImportListSync`] over the live source factory
/// (a `reqwest`-backed TMDb/IMDb/collection fetcher; Trakt/Plex are credential-
/// gated and report a graceful failed fetch).
///
/// The factory is owned so each sync resolves the lists + sources freshly per call
/// (CRUD writes take effect with no restart). The empty-vs-failed safeguard lives
/// inside `ImportListSync`, so this seam adds no library-mutating logic of its own.
pub struct LiveImportListSync {
    db: Database,
    factory: std::sync::Arc<cellarr_jobs::importlists::sources::LiveSourceFactory>,
}

impl LiveImportListSync {
    /// Build the import-list sync seam over the persistence handle, with the live
    /// `reqwest`-backed source factory.
    #[must_use]
    pub fn new(db: Database) -> Self {
        Self {
            db,
            factory: std::sync::Arc::new(
                cellarr_jobs::importlists::sources::LiveSourceFactory::default(),
            ),
        }
    }

    /// Build the db-backed sync orchestrator for this call.
    fn sync(&self) -> ImportListSync {
        ImportListSync::new(self.db.clone(), std::sync::Arc::clone(&self.factory) as _)
    }
}

#[async_trait]
impl cellarr_api::import_list_sync::ImportListSyncRunner for LiveImportListSync {
    async fn sync_all(
        &self,
    ) -> Result<cellarr_api::import_list_sync::ImportListSyncOutcome, String> {
        use cellarr_api::import_list_sync::ImportListSyncOutcome;
        let reports = self
            .sync()
            .sync_all()
            .await
            .map_err(|e| format!("import-list sync failed: {e}"))?;
        Ok(ImportListSyncOutcome::Ran(reports))
    }

    async fn sync_one(
        &self,
        list_id: &str,
    ) -> Result<cellarr_api::import_list_sync::ImportListSyncOutcome, String> {
        use cellarr_api::import_list_sync::ImportListSyncOutcome;
        use cellarr_core::ImportListRepository;
        // Resolve the list config; an unknown id is reported as an empty run (the
        // shim maps that to a 404), never an error.
        let Some(list) = ImportListRepository::get(&self.db.import_lists(), list_id)
            .await
            .map_err(|e| format!("loading import list failed: {e}"))?
        else {
            return Ok(ImportListSyncOutcome::Ran(Vec::new()));
        };
        let report = self
            .sync()
            .sync_one(&list)
            .await
            .map_err(|e| format!("import-list sync failed: {e}"))?;
        Ok(ImportListSyncOutcome::Ran(vec![report]))
    }
}

// ---------------------------------------------------------------------------
// The queue download-client seam (DELETE /api/v3/queue/{id}?removeFromClient=).
// ---------------------------------------------------------------------------

/// The daemon's
/// [`QueueDownloadClient`](cellarr_api::queue::QueueDownloadClient) implementation:
/// removes a download from its client when a queue item is removed with
/// `removeFromClient=true`.
///
/// It resolves the highest-priority enabled download client freshly per call
/// (matching the rest of the live pipeline) and calls
/// [`DownloadClient::remove`](cellarr_core::DownloadClient::remove). A
/// misconfigured/absent client is a benign error string the queue handler logs —
/// the queue row is still removed.
pub struct LiveQueueClient {
    env: LivePipelineEnv,
}

impl LiveQueueClient {
    /// Build the queue download-client seam over the persistence handle.
    #[must_use]
    pub fn new(db: Database) -> Self {
        Self {
            env: LivePipelineEnv::new(db),
        }
    }
}

#[async_trait]
impl cellarr_api::queue::QueueDownloadClient for LiveQueueClient {
    async fn remove(&self, download_id: &str, delete_data: bool) -> Result<(), String> {
        // Queue removal is not content-scoped: pick any enabled client (the
        // global selection — empty content tags matches every untagged client).
        let Some((client, _category)) = self.env.resolve_client(&[]).await? else {
            return Err("no enabled download client is configured to remove from".to_string());
        };
        client
            .remove(download_id, delete_data)
            .await
            .map_err(|e| format!("download client removal failed: {e}"))
    }

    async fn progress(
        &self,
        download_id: &str,
    ) -> Result<Option<cellarr_api::queue::QueueItemProgress>, String> {
        use cellarr_api::queue::{QueueDownloadState, QueueItemProgress};
        use cellarr_core::DownloadState;

        let Some((client, _category)) = self.env.resolve_client(&[]).await? else {
            // No client wired — the queue falls back to the stored grab status.
            return Ok(None);
        };
        let status = match client.status(download_id).await {
            Ok(s) => s,
            // The client no longer knows this id (removed), or a transient read
            // error: report "no live data" so the queue degrades to the stored
            // status rather than erroring the whole list.
            Err(_) => return Ok(None),
        };
        let state = match status.state {
            DownloadState::Queued => QueueDownloadState::Queued,
            DownloadState::Downloading => QueueDownloadState::Downloading,
            DownloadState::Completed => QueueDownloadState::Completed,
            DownloadState::Failed => QueueDownloadState::Failed,
        };
        Ok(Some(QueueItemProgress {
            state,
            progress: status.progress,
            // The download client reports a fraction + peers, not byte totals, so
            // the byte fields stay None (the queue renders the percentage + peers).
            total_bytes: None,
            size_left: None,
            peers: status.peers,
            error: status.error_string,
        }))
    }
}

#[cfg(test)]
mod auto_onboard_tests {
    use super::{native_external_id, pick_confident_candidate};
    use cellarr_api::LookupCandidate;
    use cellarr_core::MediaType;

    fn cand(title: &str, year: Option<u16>, ids: &[(&str, &str)]) -> LookupCandidate {
        LookupCandidate {
            source_id: "1".to_string(),
            media_type: MediaType::Movie,
            title: title.to_string(),
            year,
            overview: None,
            external_ids: ids
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    #[test]
    fn confident_only_on_a_single_exact_title_year_match() {
        let results = vec![cand("The Matrix", Some(1999), &[("tmdb", "603")])];
        // Exact title + year → confident.
        assert!(pick_confident_candidate(&results, "The Matrix", Some(1999)).is_some());
        // Normalization ignores case/punctuation.
        assert!(pick_confident_candidate(&results, "the matrix!", Some(1999)).is_some());
        // Year within 1 (festival vs release year) → still confident.
        assert!(pick_confident_candidate(&results, "The Matrix", Some(2000)).is_some());
        assert!(pick_confident_candidate(&results, "The Matrix", Some(1998)).is_some());
        // Off by more than 1 → not confident.
        assert!(pick_confident_candidate(&results, "The Matrix", Some(2003)).is_none());
        // A different title → no match.
        assert!(pick_confident_candidate(&results, "Blade Runner", Some(1999)).is_none());

        // Exact year wins over an adjacent-year duplicate — the ±1 fallback must not
        // turn a clean exact hit into false ambiguity.
        let dup = vec![
            cand("Abigail", Some(2024), &[("tmdb", "1")]),
            cand("Abigail", Some(2023), &[("tmdb", "2")]),
        ];
        assert_eq!(
            pick_confident_candidate(&dup, "Abigail", Some(2024)).and_then(|c| c.year),
            Some(2024)
        );
    }

    #[test]
    fn ambiguous_or_yearless_candidate_is_not_confident() {
        // Two same-title/year candidates → ambiguous → skip.
        let two = vec![
            cand("Dune", Some(2021), &[("tmdb", "438631")]),
            cand("Dune", Some(2021), &[("tmdb", "999")]),
        ];
        assert!(pick_confident_candidate(&two, "Dune", Some(2021)).is_none());

        // Parsed a year, but the candidate has none → not confident.
        let no_year = vec![cand("Dune", None, &[("tmdb", "438631")])];
        assert!(pick_confident_candidate(&no_year, "Dune", Some(2021)).is_none());

        // No parsed year → a single title match is enough.
        assert!(pick_confident_candidate(&no_year, "Dune", None).is_some());
    }

    #[test]
    fn native_external_id_prefers_the_media_type_namespace() {
        let tv = cand("Show", Some(2010), &[("tmdb", "1"), ("tvdb", "2"), ("imdb", "tt3")]);
        assert_eq!(native_external_id(&tv, MediaType::Tv), Some(("tvdb", "2")));
        assert_eq!(native_external_id(&tv, MediaType::Movie), Some(("tmdb", "1")));

        // Falls back to imdb when no numeric id is present.
        let only_imdb = cand("Movie", None, &[("imdb", "tt9")]);
        assert_eq!(native_external_id(&only_imdb, MediaType::Movie), Some(("imdb", "tt9")));

        // No ids at all → None.
        let none = cand("Bare", None, &[]);
        assert_eq!(native_external_id(&none, MediaType::Movie), None);
    }
}
