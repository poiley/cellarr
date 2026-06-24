//! The pipeline runner: advances one candidate through the `cellarr-core`
//! [`Stage`] machine, delegating every type-specific step to the media module.
//!
//! This is the executor half of `docs/03-pipeline.md`. The *rules* (legal
//! transitions) live in `cellarr-core`; the runner performs the *work* at each
//! stage and records, at every transition, a [`DecisionLogRecord`] (why) and —
//! at terminal/grab outcomes — a [`HistoryRecord`] (what). It never branches on
//! [`cellarr_core::MediaType`]: it looks up the matching [`DynMediaModule`] and
//! delegates Identify and naming to it.
//!
//! ## Seams
//!
//! The runner is constructed from the integration seams it orchestrates so it is
//! offline-testable with fakes: an [`Indexer`] (Discover), a [`DownloadClient`]
//! (Grab/Track), a [`MediaRegistry`] (Identify/Rename), a [`QualityProfile`] +
//! [`CustomFormat`]s + [`QualityRanking`] (Decide), a cellarr-fs target root
//! (Import/Rename), and a [`Database`] (decision_log + history + grab
//! persistence). Real cellarr-parse and cellarr-decide are called directly.
//!
//! ## Failure transitions are explicit
//!
//! Each stage's failure takes a *logged* transition, never a silent drop:
//! - Decide → [`Stage::Rejected`] for a normal reject (with the [`Decision`]).
//! - Grab/Track failures → [`Stage::Failed`] with a `grab-failed` note; the grab
//!   row is moved to [`GrabStatus::Blocklisted`], the release is added to the
//!   blocklist, and the dead download is removed from the client (with its local
//!   data) so a re-search never re-grabs the same bad release.
//! - Import failures → [`Stage::HeldForReview`] (`import-failed → hold`).
//!
//! ## Failure-resilient acquisition (self-heal)
//!
//! A download that fails — or **stalls** (no progress and zero peers for a bounded
//! number of polls, see [`STALL_MAX_STAGNANT_POLLS`]) — does not end the run. The
//! runner blocklists the release, removes it from the client, and **grabs the next
//! best non-blocklisted candidate in the same run**, repeating up to
//! [`MAX_GRAB_NEXT_ATTEMPTS`] times (and stopping when no candidate remains). This
//! is the bounded "grab next release on failure" self-healing Sonarr/Radarr do.

use std::sync::Arc;

use cellarr_core::blocklist::{BlocklistEntry, BlocklistRepository};
use cellarr_core::{
    decision::Verdict,
    history::{DecisionLogRecord, HistoryEvent, HistoryRecord},
    pipeline::{Stage, Transition, TransitionKind},
    ContentMatch, ContentRef, CustomFormat, Decision, DownloadState, GrabId, GrabRequest,
    GrabStatus, IndexerId, NamingTokens, ParsedRelease, PipelineRunId, PlannedMove, QualityProfile,
    QualityRanking, Release, Score,
};
use cellarr_core::{
    repo::{DecisionLogRepository, GrabRepository, HistoryRepository},
    traits::{DownloadClient, Indexer},
};
use cellarr_core::{WebhookEventType, WebhookFile, WebhookPayload, WebhookSubject};
use cellarr_db::Database;
use cellarr_decide::{decide, DecisionContext, OnDiskFile, ProperRepackPolicy};
use cellarr_media::MediaRegistry;
use time::OffsetDateTime;

use crate::clock::Clock;
use crate::error::{BoxError, JobError, Result};
use crate::notify::{ProviderNotifier, WebhookNotifier};
use cellarr_core::{
    NotificationEvent, NotificationMessage, NotificationRelease, NotificationSubject,
};

/// How many consecutive poll cycles a download may sit with **stagnant progress
/// and zero connected peers** before the runner treats it as a dead torrent and
/// fails it (a *stall*). A torrent making no progress with nobody to download
/// from will never complete; failing it after a bounded wait lets the self-heal
/// path blocklist it and grab the next release rather than waiting out the whole
/// `max_track_polls` budget.
///
/// The signal is conservative: a stall requires the client to *report* zero peers
/// (`Some(0)`); an unknown peer count (`None`, e.g. Usenet) never trips it, and
/// any forward progress resets the counter. A **near-complete** download
/// ([`NEAR_COMPLETE_PROGRESS`]) is also exempt — see that constant. Default 3 cycles.
const STALL_MAX_STAGNANT_POLLS: u32 = 3;

/// A download at or above this fraction is treated as **near-complete** and is
/// exempt from the stall detector. At a torrent's end-game the client commonly
/// reports `progress ≈ 0.999` (the final piece is still verifying) while its peer
/// count naturally drops to zero — which looks exactly like a stall ("no progress,
/// no peers") even though the content is essentially done and about to flip to
/// `Completed`. Killing it there would discard a finished download moments before
/// import. So once a download crosses this line we stop counting stalls and let it
/// finish (it will reach `Completed`, or eventually exhaust `max_track_polls` if it
/// genuinely hangs on a last unavailable piece). Observed live against Transmission.
const NEAR_COMPLETE_PROGRESS: f32 = 0.99;

/// The maximum number of grab-next iterations the runner performs for one content
/// node in a single run before giving up. After a download fails (or stalls) the
/// runner blocklists the release, removes it from the client, and re-decides over
/// the remaining (non-blocklisted) candidates to grab the next-best — capped here
/// so a content with many failing releases can never loop unboundedly. The loop
/// also stops naturally as soon as no non-blocklisted candidate remains. Mirrors
/// Sonarr/Radarr's bounded "grab next release on failure". Default 5.
const MAX_GRAB_NEXT_ATTEMPTS: u32 = 5;

/// The result of tracking one download to a terminal point.
enum TrackOutcome {
    /// The download completed; carries the client-reported content path (if any).
    Completed(Option<String>),
    /// The download terminated in a failure (hard failure, stall, or exhausted
    /// polls); carries the human detail used for the blocklist reason + log.
    Failed(String),
}

/// One candidate that passed Parse/Identify/Decide and is ready to grab.
struct PickedCandidate {
    matched_ref: ContentRef,
    release: Release,
    parsed: ParsedRelease,
    decision: Decision,
    score: Score,
}

/// What walking the candidate list for a grabbable release yielded.
enum PickResult {
    /// A candidate is ready to grab. Boxed: it carries the (large) parsed release
    /// + decision, which would otherwise bloat every variant of this enum.
    Grabbable(Box<PickedCandidate>),
    /// The run must be held for manual resolution (an unresolved anime absolute or
    /// an identify failure) — never grab-next over a node we cannot place safely.
    Held(String),
    /// No grabbable candidate remains (all rejected/blocklisted/unmatched).
    None,
}

/// The outcome of one grab→track→import attempt.
enum GrabTrackResult {
    /// A terminal outcome for the whole run (imported, or held for review).
    Done(RunOutcome),
    /// The download failed or stalled; the release was blocklisted and removed
    /// from the client. The caller should grab the next-best release. Carries the
    /// failure detail for the run's terminal `Failed` outcome if the loop ends.
    FailedGrabNext(String),
}

/// The terminal outcome of driving a candidate through the pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// A release was grabbed, downloaded, and imported. Carries the grab id and
    /// the destination paths the files landed at.
    Imported {
        /// The grab that completed.
        grab_id: GrabId,
        /// The on-disk destination paths, in plan order.
        destinations: Vec<String>,
    },
    /// The candidate was rejected at Decide (a normal, logged outcome).
    Rejected {
        /// The machine-readable reason, as rendered for the log.
        reason: String,
    },
    /// A download or grab failed; the release was failed/blocklisted and the run
    /// ended at [`Stage::Failed`].
    Failed {
        /// What failed.
        detail: String,
    },
    /// The import could not be safely committed and was held for the user.
    HeldForReview {
        /// Why it was held.
        reason: String,
    },
    /// No acceptable release was found at Discover.
    NothingFound,
}

/// One ranked candidate from an **interactive (manual) release search** — the
/// read-only Discover→Parse→Identify→Decide preview that powers the
/// `GET /api/v3/release` interactive-search screen.
///
/// Unlike a pipeline run, producing these performs **no grab**: every discovered
/// release is parsed, identified, and scored, and the resulting verdict is
/// reported so the UI can show what *would* be grabbed (and why each rejected
/// release was rejected) and let the user grab one by hand. It mirrors the
/// Sonarr/Radarr interactive-search row: identity + quality + score + the
/// rejected/reason flags.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseCandidate {
    /// The candidate release as advertised by the indexer (carries the guid,
    /// title, protocol, size, seeders the UI renders and a later grab uses).
    pub release: Release,
    /// The resolved quality (name + rank) the candidate was graded on, from
    /// cellarr's quality catalogue. The Unknown sentinel (rank 0) is used for a
    /// title the parser could not bucket.
    pub quality: cellarr_core::Quality,
    /// The total custom-format score the decision engine computed for the
    /// candidate (0 when no custom formats match).
    pub custom_format_score: i32,
    /// Whether the candidate was rejected (not grabbable as-is). A non-rejected
    /// candidate is one the pipeline would Grab or accept as an Upgrade.
    pub rejected: bool,
    /// A human-readable reason: why it was rejected, or — when accepted — the
    /// grab/upgrade rationale. Always populated so the UI never shows a blank
    /// row.
    pub reason: String,
}

/// One file found by a **manual-import scan** of a loose folder — parsed,
/// size-stamped, and (when it confidently identifies) suggested onto a content
/// node. This is the read-only preview the `GET /api/v3/manualimport` screen
/// renders before the user commits an import; producing it **moves nothing**.
///
/// Mirrors the Sonarr/Radarr manual-import row: the source path/name/size, the
/// parsed quality, the suggested placement (content id + season/episode), and any
/// rejections (a file the scanner cannot place, e.g. one that did not parse or
/// did not confidently identify, still appears so the user can map it by hand).
#[derive(Debug, Clone, PartialEq)]
pub struct ManualImportCandidate {
    /// Absolute path of the loose file on disk.
    pub path: String,
    /// The file's base name (what the row labels itself with).
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// The cleaned title the parser extracted, when it found one.
    pub parsed_title: Option<String>,
    /// The resolved quality the file parsed to (Unknown sentinel when the parser
    /// could not bucket it).
    pub quality: cellarr_core::Quality,
    /// The content node the scanner suggests this file be imported onto, when it
    /// confidently identified one. `None` means the user must pick a node by hand
    /// (the row carries a rejection explaining why).
    pub suggested: Option<ManualImportSuggestion>,
    /// Per-file reasons the scanner could not auto-place the file (did not parse,
    /// did not identify, ambiguous). Empty when `suggested` is `Some`.
    pub rejections: Vec<String>,
}

/// The content placement a manual-import scan suggests for one file: the node id
/// plus, for TV, the season/episode the file parsed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualImportSuggestion {
    /// The suggested content node.
    pub content_id: cellarr_core::ContentId,
    /// The season number, for a TV file.
    pub season: Option<u32>,
    /// The episode number, for a TV file.
    pub episode: Option<u32>,
}

/// One user-chosen file to import, as committed through
/// [`PipelineRunner::import_manual`]. The user picked the file and the content
/// node it maps to (overriding or confirming the scan's suggestion); the runner
/// drives it through the **same crash-safe stage→verify→commit→log import path** a
/// pipeline run uses — it never moves a byte until the plan is verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualImportRequest {
    /// Absolute path of the loose source file to import.
    pub path: String,
    /// The content node the user chose to import the file onto.
    pub content_id: cellarr_core::ContentId,
}

/// The outcome of importing one [`ManualImportRequest`]: where the file landed
/// (renamed, under the library root) and the content node it was linked to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualImportResult {
    /// The original source path the user asked to import.
    pub source_path: String,
    /// The on-disk destination the file was renamed/moved to.
    pub destination_path: String,
    /// The content node the imported file was linked to.
    pub content_id: cellarr_core::ContentId,
}

/// Everything the runner needs that is *not* a live integration seam: the
/// decision inputs and the on-disk naming/target configuration.
///
/// Bundled so [`PipelineRunner::run`] keeps a small signature; built once per
/// library/profile and reused across runs.
pub struct RunnerConfig {
    /// The active quality profile.
    pub profile: QualityProfile,
    /// The custom formats to score against.
    pub custom_formats: Vec<CustomFormat>,
    /// The quality catalogue candidates are ranked against.
    pub ranking: QualityRanking,
    /// Proper/repack handling for the decision engine.
    pub proper_repack_policy: ProperRepackPolicy,
    /// The library root the imported files are placed under.
    pub library_root: std::path::PathBuf,
    /// The naming format passed to the rename engine (cellarr-fs `render_name`),
    /// rendered against the media module's [`NamingTokens`]. The result is a
    /// relative path joined onto `library_root`.
    pub naming_format: String,
    /// The indexer id grabs are attributed to (the seam itself is type-erased).
    pub indexer_id: IndexerId,
    /// The download client id grabs are attributed to.
    pub client_id: cellarr_core::DownloadClientId,
    /// The download category cellarr tags its grabs with.
    pub category: String,
    /// How many times to poll the download client before giving up tracking.
    pub max_track_polls: u32,
    /// How long to wait between Track polls. Real deployments set a few seconds
    /// so the poll budget spans a multi-minute download; tests set
    /// [`Duration::ZERO`](std::time::Duration::ZERO) so they never sleep (the
    /// logical clock still advances per poll for retry/stall accounting).
    pub track_poll_interval: std::time::Duration,
    /// The download client's host, used to scope which [remote-path
    /// mappings](RunnerConfig::remote_path_mappings) apply (the Sonarr/Radarr
    /// convention: a mapping names the client host it rewrites paths for). Empty
    /// when the client runs alongside cellarr (no mapping needed).
    #[allow(clippy::doc_markdown)]
    pub client_host: String,
    /// Remote-path mappings applied — in one shared place — to the client's
    /// reported `content_path` before Import, so a path the client reports under
    /// its own mount (`/downloads/x`) is rewritten to where cellarr sees it
    /// (`/data/downloads/x`). Empty (the default) is a no-op. This is the single
    /// site the rewrite happens regardless of which download client produced the
    /// path; see [`cellarr_core::apply_remote_path_mappings`].
    pub remote_path_mappings: Vec<cellarr_core::RemotePathMapping>,
    /// Whether to write Kodi/Jellyfin-compatible `.nfo` metadata sidecars next to
    /// imported media (the media-management "write metadata" setting). Default on.
    /// Sidecars are written best-effort *after* the media is durably committed, so
    /// disabling this never changes the crash-safe import guarantee.
    pub write_nfo: bool,
}

/// Drives candidates through the pipeline state machine.
///
/// Holds borrowed seams for the duration of a run; cheap to construct per run.
pub struct PipelineRunner<'a, I, D, C>
where
    I: Indexer,
    D: DownloadClient,
    C: Clock,
{
    indexer: &'a I,
    client: &'a D,
    registry: &'a MediaRegistry,
    db: &'a Database,
    clock: &'a C,
    config: &'a RunnerConfig,
    /// The Connect-webhook dispatcher, fired at the Grab/Import/Rename
    /// transitions. `None` (the default) sends nothing — the offline/test path —
    /// so the pipeline never depends on webhook wiring.
    notifier: Option<WebhookNotifier>,
    /// The broadened notification-provider dispatcher (Discord/Telegram/Email/
    /// Custom Script/media-server rescans/provider webhook), fired at the same
    /// Grab/Import/Upgrade transitions as the Connect webhook. `None` (the
    /// default) sends nothing, so the offline/test path depends on no provider
    /// wiring; live wiring opts in via [`with_provider_notifier`](Self::with_provider_notifier).
    provider_notifier: Option<ProviderNotifier>,
    /// The anime scene-mapping provider, used at Identify to remap an absolute
    /// episode number to its season/episode (TheXEM, behind the seam). `None`
    /// (the default) means no remap is attempted — a release that still carries an
    /// absolute coordinate is then surfaced for manual resolution rather than
    /// guessed, so the absence of a provider is safe.
    scene_provider: Option<Arc<dyn cellarr_media::DynSceneMappingProvider>>,
    /// The content-metadata resolver, called once a release confidently identifies
    /// to a node so the resolved facts (year/overview/runtime/air-date) and artwork
    /// are persisted on that node. `None` (the default) means no metadata is
    /// persisted at Identify — the offline/test path — and acquisition proceeds
    /// unaffected (metadata persistence is best-effort, never a pipeline gate).
    metadata_resolver: Option<Arc<dyn cellarr_media::DynMetadataResolver>>,
}

impl<'a, I, D, C> PipelineRunner<'a, I, D, C>
where
    I: Indexer,
    D: DownloadClient,
    C: Clock,
{
    /// Construct a runner over its seams.
    pub fn new(
        indexer: &'a I,
        client: &'a D,
        registry: &'a MediaRegistry,
        db: &'a Database,
        clock: &'a C,
        config: &'a RunnerConfig,
    ) -> Self {
        Self {
            indexer,
            client,
            registry,
            db,
            clock,
            config,
            notifier: None,
            provider_notifier: None,
            scene_provider: None,
            metadata_resolver: None,
        }
    }

    /// Attach a content-metadata resolver so a confidently-identified node has its
    /// resolved facts (year/overview/runtime/air-date) and artwork persisted at
    /// Identify. Builder form so the base [`PipelineRunner::new`] stays offline (no
    /// metadata persistence) and live wiring opts in. Persistence is best-effort:
    /// a resolver failure is logged and the run proceeds.
    #[must_use]
    pub fn with_metadata_resolver(
        mut self,
        resolver: Arc<dyn cellarr_media::DynMetadataResolver>,
    ) -> Self {
        self.metadata_resolver = Some(resolver);
        self
    }

    /// Attach an anime scene-mapping provider so the Identify path remaps an
    /// absolute episode number to its season/episode (the TheXEM call-site).
    /// Builder form so the base [`PipelineRunner::new`] stays offline (no remap,
    /// absolute releases surfaced for manual resolution) and live wiring opts in.
    #[must_use]
    pub fn with_scene_provider(
        mut self,
        provider: Arc<dyn cellarr_media::DynSceneMappingProvider>,
    ) -> Self {
        self.scene_provider = Some(provider);
        self
    }

    /// Attach a Connect-webhook dispatcher so Grab/Import/Rename transitions fire
    /// `eventType` webhooks to the configured notifications. Builder form so the
    /// base [`PipelineRunner::new`] stays offline (no webhooks) and the live
    /// wiring opts in.
    #[must_use]
    pub fn with_notifier(mut self, notifier: WebhookNotifier) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// Attach the broadened notification-provider dispatcher so Grab/Import/
    /// Upgrade transitions also fire Discord/Telegram/Email/Custom-Script and the
    /// media-server rescan providers. Builder form so the base
    /// [`PipelineRunner::new`] stays offline (no providers) and live wiring opts
    /// in. Best-effort, exactly like the Connect webhook: a provider failure is
    /// logged inside the dispatcher and never affects the run.
    #[must_use]
    pub fn with_provider_notifier(mut self, notifier: ProviderNotifier) -> Self {
        self.provider_notifier = Some(notifier);
        self
    }

    /// Run the full pipeline for `content`, returning the terminal outcome.
    ///
    /// Advances `Discover → Parse → Identify → Decide → Grab → Track → Import →
    /// Rename → Notify → Done`, taking the logged reject/fail/hold branch where a
    /// stage so determines. Every transition appends a decision-log record; grab,
    /// completion, import, failure, and hold also append history.
    ///
    /// # Errors
    /// Returns [`JobError`] only for *infrastructure* failures the run cannot
    /// recover from (a repository write failed, an illegal transition was
    /// constructed). Domain outcomes — reject, grab-failed, import-held — are
    /// returned as [`RunOutcome`], not errors, because they are normal, logged
    /// results, not bugs.
    pub async fn run(&self, content: &ContentRef) -> Result<RunOutcome> {
        let run_id = PipelineRunId::new();
        let mut stage = Stage::Discover;

        // --- Discover -----------------------------------------------------
        let releases = self.discover(content).await?;
        if releases.is_empty() {
            self.log(
                run_id,
                stage,
                Stage::Failed,
                TransitionKind::Fail,
                None,
                Some("no releases found".into()),
            )
            .await?;
            return Ok(RunOutcome::NothingFound);
        }
        stage = self.advance(run_id, stage, None).await?; // -> Parse

        // The grab-next self-heal loop: pick the best non-blocklisted candidate,
        // grab+track+import it, and — if the download FAILS or STALLS — blocklist
        // it, remove it from the client, and re-decide over the remaining
        // candidates to grab the *next*-best. Bounded by [`MAX_GRAB_NEXT_ATTEMPTS`]
        // so a content whose every release fails can never loop forever; it also
        // stops as soon as no non-blocklisted grabbable candidate remains. This
        // mirrors Sonarr/Radarr's "grab next release on failure" self-healing.
        let mut last_reject: Option<String> = None;
        let mut last_failure: Option<String> = None;
        for _attempt in 0..MAX_GRAB_NEXT_ATTEMPTS {
            // Walk the candidates for the best grabbable one this attempt. A
            // blocklisted release (including one this run just failed) is skipped;
            // a reject is logged and the next candidate tried.
            let picked = match self
                .pick_grabbable(run_id, &mut stage, content, &releases, &mut last_reject)
                .await?
            {
                PickResult::Grabbable(picked) => picked,
                // An anime absolute that cannot be resolved (or an identify
                // failure) holds the whole run for manual resolution — never a
                // guess, never a grab-next over a node we cannot place safely.
                PickResult::Held(reason) => return Ok(RunOutcome::HeldForReview { reason }),
                // No grabbable candidate remains this attempt: stop the loop and
                // return the last logged reject/failure below.
                PickResult::None => break,
            };

            match self
                .grab_track_import(
                    run_id,
                    &mut stage,
                    content,
                    &picked.matched_ref,
                    &picked.release,
                    &picked.parsed,
                    picked.decision,
                    picked.score,
                )
                .await?
            {
                // Imported or held-for-review: a terminal outcome for this run.
                GrabTrackResult::Done(outcome) => return Ok(outcome),
                // The download failed/stalled and was blocklisted + removed from
                // the client. Loop again to grab the next-best release (the failed
                // one is now blocklisted, so the next walk skips it).
                GrabTrackResult::FailedGrabNext(detail) => {
                    tracing::warn!(
                        content = %content.id,
                        detail = %detail,
                        "download failed; blocklisted and grabbing next release"
                    );
                    last_failure = Some(detail);
                    // The failed attempt logged its own Grab->Failed transition; a
                    // grab-next is a fresh Decide->Grab cycle under the same run.
                    // Reset the local stage cursor to Decide so the next attempt's
                    // advances are legal (the decision log is an append-only event
                    // stream, so several grab cycles in one run read correctly).
                    stage = Stage::Decide;
                    continue;
                }
            }
        }

        // No candidate imported within the attempt budget. If at least one grab
        // failed, the run's terminal state is Failed (every grabbed release died);
        // otherwise it is a normal Rejected (nothing acceptable was offered).
        if let Some(detail) = last_failure {
            return Ok(RunOutcome::Failed { detail });
        }
        let reason = last_reject.unwrap_or_else(|| "no acceptable release".into());
        Ok(RunOutcome::Rejected { reason })
    }

    /// Run the **read-only interactive search** for `content`: Discover→Parse→
    /// Identify→Decide every release the configured indexers offer, returning the
    /// scored candidates **without grabbing any of them**.
    ///
    /// This is the engine behind `GET /api/v3/release` (the interactive-search
    /// screen). It reuses the exact same Discover, parse, identify, blocklist, and
    /// Decide steps a real run takes — so the score, quality, and reject reason a
    /// user sees match what the pipeline would actually do — but stops short of the
    /// Grab stage and writes nothing to the decision log or history (a preview is
    /// not a pipeline run).
    ///
    /// Candidates are returned ranked best-first: grabbable releases (Grab /
    /// Upgrade) before rejected ones, each group ordered by quality rank then
    /// custom-format score, descending — the order the interactive UI lists them
    /// in. A release that does not confidently identify to `content` is dropped
    /// (it is not a candidate for this node). An anime absolute that cannot be
    /// resolved, or an identify error, is skipped rather than aborting the whole
    /// search, so one un-placeable release never blanks the screen.
    ///
    /// # Errors
    /// Returns [`JobError`] only for infrastructure failures (a Discover seam
    /// error, or a repository read failure). Per-release domain outcomes —
    /// reject, unmatched — are carried in the returned candidates, never errored.
    pub async fn preview_releases(&self, content: &ContentRef) -> Result<Vec<ReleaseCandidate>> {
        let releases = self.discover(content).await?;
        let mut out: Vec<ReleaseCandidate> = Vec::new();

        for release in &releases {
            // Parse the advertised title (real cellarr-parse), then reconcile any
            // anime absolute coordinate the same way a real run does. An
            // unresolvable absolute (or identify failure) is skipped for the
            // preview rather than held — the user is browsing candidates, not
            // committing a placement.
            let parsed = cellarr_parse::parse_title(&release.title);
            let parsed = match self.remap_absolute_coords(content, parsed).await {
                Ok(p) => p,
                Err(_) => continue,
            };
            let matches = match self.identify(content, &parsed).await {
                Ok(m) => m,
                Err(_) => continue,
            };
            let Some(matched) = self.best_match(content, matches) else {
                // Did not confidently identify to this node: not a candidate.
                continue;
            };

            // The quality the candidate is graded on, from cellarr's catalogue.
            let quality = cellarr_core::resolve_quality(&parsed, &self.config.ranking);

            // A blocklisted release is reported as rejected (so the user sees it
            // and why) but never grabbable.
            if self.is_blocklisted(matched.content_ref.id, release).await? {
                out.push(ReleaseCandidate {
                    release: release.clone(),
                    quality,
                    custom_format_score: 0,
                    rejected: true,
                    reason: "release is blocklisted".to_string(),
                });
                continue;
            }

            // Decide (real cellarr-decide) against the current on-disk file, so
            // the verdict and score match what a real run would compute.
            let on_disk = self.on_disk_for(content).await?;
            let decision = self.decide(&matched.content_ref, release, &parsed, on_disk)?;
            let (rejected, cf_score, reason) = match &decision.verdict {
                // A reject verdict carries no score (the engine stops scoring once
                // it rejects), so a rejected row reports 0 — it would not be
                // grabbed regardless of its custom-format score.
                Verdict::Reject { reason } => (true, 0, reason_text(reason)),
                Verdict::Grab { score } => (false, score.custom_format_score, "grab".to_string()),
                Verdict::Upgrade { from, to, .. } => (
                    false,
                    to.custom_format_score,
                    format!(
                        "upgrade (rank {}->{}, score {}->{})",
                        from.quality_rank,
                        to.quality_rank,
                        from.custom_format_score,
                        to.custom_format_score
                    ),
                ),
            };
            out.push(ReleaseCandidate {
                release: release.clone(),
                quality,
                custom_format_score: cf_score,
                rejected,
                reason,
            });
        }

        // Rank best-first: grabbable before rejected, then by quality rank and
        // custom-format score (both descending) — the interactive-search order.
        out.sort_by(|a, b| {
            a.rejected
                .cmp(&b.rejected)
                .then(b.quality.rank.cmp(&a.quality.rank))
                .then(b.custom_format_score.cmp(&a.custom_format_score))
        });
        Ok(out)
    }

    /// **Interactive grab**: grab one specific release the user picked from the
    /// interactive-search screen and drive it through Grab→Track→Import.
    ///
    /// This is the engine behind `POST /api/v3/release`. Unlike
    /// [`preview_releases`](Self::preview_releases) (which never grabs), this path
    /// **does** build and drive the download client: it re-runs Discover to locate
    /// the release the user chose (matched by `guid`, falling back to the download
    /// URL), parses/identifies it to `content`, then hands it to the download
    /// client and tracks it to import — the exact [`grab_track_import`] path a full
    /// pipeline run takes, so a manual grab and an automatic grab are
    /// indistinguishable downstream (same grab row, history, webhook, import).
    ///
    /// The user explicitly chose this release, so the Decide verdict is **not** a
    /// gate here — a release the engine would have rejected for scoring (e.g. lower
    /// than the on-disk file) is still grabbable by hand, mirroring Sonarr/Radarr's
    /// "override and grab" on the interactive screen. A *blocklisted* release is
    /// still refused (it previously failed for this node), and a release that does
    /// not confidently identify to `content` is refused (we will not place a file
    /// on the wrong node — the library-safety rule).
    ///
    /// # Errors
    /// Returns [`JobError`] for infrastructure failures (Discover seam error,
    /// repository failure). A release that cannot be found, identified, or is
    /// blocklisted is reported as a domain [`RunOutcome`]
    /// ([`Rejected`](RunOutcome::Rejected) / [`HeldForReview`](RunOutcome::HeldForReview)),
    /// never an error.
    ///
    /// [`grab_track_import`]: Self::grab_track_import
    pub async fn grab_release(&self, content: &ContentRef, guid: &str) -> Result<RunOutcome> {
        let run_id = PipelineRunId::new();
        let mut stage = Stage::Discover;

        // Re-discover to locate the chosen release. The interactive search the user
        // grabbed from listed the live candidates; we re-run Discover and match the
        // user's pick by guid (the stable id the UI carries), falling back to the
        // download URL when an indexer advertises no guid.
        let releases = self.discover(content).await?;
        let Some(release) = releases
            .into_iter()
            .find(|r| r.guid.as_deref() == Some(guid) || r.download_url == guid)
        else {
            let reason = format!("release {guid} is no longer offered by the indexers");
            self.log(
                run_id,
                Stage::Discover,
                Stage::Failed,
                TransitionKind::Fail,
                None,
                Some(reason.clone()),
            )
            .await?;
            return Ok(RunOutcome::NothingFound);
        };
        stage = self.advance(run_id, stage, None).await?; // -> Parse

        // Parse + reconcile any anime absolute, exactly as a run does. An
        // unresolvable absolute holds for manual resolution — never a guess.
        let parsed = cellarr_parse::parse_title(&release.title);
        let parsed = match self.remap_absolute_coords(content, parsed).await {
            Ok(p) => p,
            Err(reason) => {
                self.log(
                    run_id,
                    Stage::Identify,
                    Stage::HeldForReview,
                    TransitionKind::Hold,
                    None,
                    Some(reason.clone()),
                )
                .await?;
                return Ok(RunOutcome::HeldForReview { reason });
            }
        };

        // Identify to the node. A release that does not confidently match `content`
        // is refused — a manual grab still must not place a file on the wrong node.
        let matches = match self.identify(content, &parsed).await {
            Ok(m) => m,
            Err(e) => {
                let reason = format!("identify failed: {e}");
                self.log(
                    run_id,
                    Stage::Identify,
                    Stage::HeldForReview,
                    TransitionKind::Hold,
                    None,
                    Some(reason.clone()),
                )
                .await?;
                return Ok(RunOutcome::HeldForReview { reason });
            }
        };
        stage = self.advance(run_id, stage, None).await?; // -> Identify
        let Some(matched) = self.best_match(content, matches) else {
            let reason = "release does not confidently identify to this item".to_string();
            self.log(
                run_id,
                Stage::Identify,
                Stage::Rejected,
                TransitionKind::Reject,
                None,
                Some(reason.clone()),
            )
            .await?;
            return Ok(RunOutcome::Rejected { reason });
        };
        stage = self.advance(run_id, stage, None).await?; // -> Decide

        // A blocklisted release previously failed for this node; refuse it even on
        // a manual grab (grab-next blocklisted it for a reason).
        if self
            .is_blocklisted(matched.content_ref.id, &release)
            .await?
        {
            let reason = "release is blocklisted".to_string();
            self.log(
                run_id,
                Stage::Decide,
                Stage::Rejected,
                TransitionKind::Reject,
                None,
                Some(reason.clone()),
            )
            .await?;
            return Ok(RunOutcome::Rejected { reason });
        }

        // Decide is run for its log + score, but the verdict does NOT gate a manual
        // grab: the user explicitly chose this release. A Reject verdict is grabbed
        // anyway (recorded as a manual override); Grab/Upgrade carry their score.
        let on_disk = self.on_disk_for(content).await?;
        let decision = self.decide(&matched.content_ref, &release, &parsed, on_disk)?;
        let score = match &decision.verdict {
            Verdict::Grab { score } | Verdict::Upgrade { to: score, .. } => *score,
            // A manually-overridden grab of a release the engine would reject: use a
            // zero score (the user is overriding scoring, not relying on it).
            Verdict::Reject { .. } => Score::default(),
        };

        match self
            .grab_track_import(
                run_id,
                &mut stage,
                content,
                &matched.content_ref,
                &release,
                &parsed,
                decision,
                score,
            )
            .await?
        {
            GrabTrackResult::Done(outcome) => Ok(outcome),
            GrabTrackResult::FailedGrabNext(detail) => Ok(RunOutcome::Failed { detail }),
        }
    }

    /// Walk the discovered `releases` for the best **grabbable** candidate this
    /// attempt: the first that identifies to `content`, is not blocklisted, and
    /// whose decision is Grab/Upgrade. Rejects and blocklisted/unmatched
    /// candidates are logged and skipped (recording the last reject for the
    /// caller). Returns `Ok(None)` when no candidate is grabbable.
    async fn pick_grabbable(
        &self,
        run_id: PipelineRunId,
        stage: &mut Stage,
        content: &ContentRef,
        releases: &[Release],
        last_reject: &mut Option<String>,
    ) -> Result<PickResult> {
        for release in releases {
            // --- Parse (real cellarr-parse on the *title*) ----------------
            let parsed = cellarr_parse::parse_title(&release.title);

            // --- Identify: anime absolute->episode remap (the XEM call-site) --
            // An anime release advertises an *absolute* episode number; the
            // library is addressed by season/episode. Reconcile it here, via the
            // series' scene mapping, BEFORE delegating to the media module (which
            // only matches canonical Episode coordinates). An unmapped absolute is
            // surfaced for manual resolution — never guessed (library-safety).
            let parsed = match self.remap_absolute_coords(content, parsed).await {
                Ok(p) => p,
                Err(reason) => {
                    self.log(
                        run_id,
                        Stage::Identify,
                        Stage::HeldForReview,
                        TransitionKind::Hold,
                        None,
                        Some(reason.clone()),
                    )
                    .await?;
                    return Ok(PickResult::Held(reason));
                }
            };

            // --- Identify (delegated to the media module) -----------------
            let matches = match self.identify(content, &parsed).await {
                Ok(m) => m,
                Err(e) => {
                    let reason = format!("identify failed: {e}");
                    self.log(
                        run_id,
                        Stage::Identify,
                        Stage::HeldForReview,
                        TransitionKind::Hold,
                        None,
                        Some(reason.clone()),
                    )
                    .await?;
                    return Ok(PickResult::Held(reason));
                }
            };
            if *stage == Stage::Parse {
                *stage = self.advance(run_id, *stage, None).await?; // -> Identify
            }
            let Some(matched) = self.best_match(content, matches) else {
                *last_reject = Some("no confident content match".into());
                continue;
            };
            // A confident identify is where the node's rich metadata becomes
            // resolvable: persist it (and cache artwork) before deciding. Best-effort.
            self.persist_metadata(&matched.content_ref).await;
            if *stage == Stage::Identify {
                *stage = self.advance(run_id, *stage, None).await?; // -> Decide
            }

            // --- Blocklist consultation (before Decide) -------------------
            // A previously-failed release for this content must never be
            // re-grabbed; skip it and try the next candidate (the
            // download-failed -> blocklist + re-search transition). This is also
            // what makes grab-next converge: the release this run just failed is
            // blocklisted, so the next walk passes over it to the next-best.
            let blocklisted = self.is_blocklisted(matched.content_ref.id, release).await?;
            if blocklisted {
                let note = "rejected: release is blocklisted".to_string();
                self.log(
                    run_id,
                    Stage::Decide,
                    Stage::Rejected,
                    TransitionKind::Reject,
                    None,
                    Some(note.clone()),
                )
                .await?;
                *last_reject = Some(note);
                continue;
            }

            // --- Decide (real cellarr-decide) -----------------------------
            let on_disk = self.on_disk_for(content).await?;
            let decision = self.decide(&matched.content_ref, release, &parsed, on_disk)?;
            match &decision.verdict {
                Verdict::Reject { reason } => {
                    let note = format!("rejected: {}", reason_text(reason));
                    self.log_decision(
                        run_id,
                        Stage::Decide,
                        Stage::Rejected,
                        TransitionKind::Reject,
                        decision.clone(),
                        Some(note.clone()),
                    )
                    .await?;
                    *last_reject = Some(note);
                    continue;
                }
                Verdict::Grab { score } | Verdict::Upgrade { to: score, .. } => {
                    let score = *score;
                    return Ok(PickResult::Grabbable(Box::new(PickedCandidate {
                        matched_ref: matched.content_ref,
                        release: release.clone(),
                        parsed,
                        decision,
                        score,
                    })));
                }
            }
        }
        Ok(PickResult::None)
    }

    // --- Stage implementations -------------------------------------------

    async fn discover(&self, content: &ContentRef) -> Result<Vec<Release>> {
        let module = self.module_for(content)?;
        let terms = module
            .search_terms(content)
            .await
            .map_err(|e| JobError::stage_boxed(Stage::Discover, e))?;
        self.indexer
            .search(&terms)
            .await
            .map_err(|e| JobError::stage(Stage::Discover, e))
    }

    /// Reconcile any anime absolute coordinate in `parsed` to a canonical
    /// season/episode via the series' scene mapping — the wired-up XEM call-site.
    ///
    /// Behavior:
    /// - No absolute coordinate, or a non-TV node: the parse is returned
    ///   unchanged (fast path; nothing to remap).
    /// - An absolute coordinate present: resolve the node's series TVDB id through
    ///   the identity-link db query, then remap through the scene provider. On
    ///   success the absolute coordinate is replaced in-place by the resolved
    ///   `Episode { season, episode, absolute: Some(n) }`.
    /// - Unresolvable (no scene provider configured, no series TVDB id linked, or
    ///   no mapping covers the number): returns `Err(reason)` so the caller holds
    ///   the run for manual resolution. The absolute is **never guessed** onto a
    ///   season/episode (the library-safety rule).
    async fn remap_absolute_coords(
        &self,
        content: &ContentRef,
        mut parsed: ParsedRelease,
    ) -> std::result::Result<ParsedRelease, String> {
        use cellarr_core::Coordinates;
        // Fast path: nothing absolute to reconcile.
        if !parsed
            .coordinates
            .iter()
            .any(|c| matches!(c, Coordinates::Absolute { .. }))
        {
            return Ok(parsed);
        }

        // No provider wired: an absolute release cannot be safely placed. Surface
        // it rather than guess.
        let Some(provider) = self.scene_provider.as_ref() else {
            return Err(
                "anime absolute release needs scene mapping but no provider is configured \
                 (surfaced for manual resolution)"
                    .to_string(),
            );
        };

        // The identity-link query that gates the remap: a content node -> its
        // series' TVDB id, read from the metadata identity tables.
        let tvdb_id = self
            .db
            .content()
            .series_tvdb_id(content.id)
            .await
            .map_err(|e| format!("series identity lookup failed: {e}"))?;
        let Some(tvdb_id) = tvdb_id else {
            return Err(
                "anime absolute release: series has no linked TVDB id, cannot resolve absolute \
                 numbering (surfaced for manual resolution)"
                    .to_string(),
            );
        };
        let series_external_id = tvdb_id.to_string();

        // Remap each absolute coordinate; anything unmapped/malformed is held.
        let mut remapped = Vec::with_capacity(parsed.coordinates.len());
        for coord in &parsed.coordinates {
            match coord {
                Coordinates::Absolute { .. } => {
                    let placed = cellarr_media::remap_absolute_dyn(
                        provider.as_ref(),
                        &series_external_id,
                        coord,
                    )
                    .await
                    .map_err(|e| {
                        format!("anime absolute remap surfaced for manual resolution: {e}")
                    })?;
                    remapped.push(placed);
                }
                other => remapped.push(other.clone()),
            }
        }
        parsed.coordinates = remapped;
        Ok(parsed)
    }

    async fn identify(
        &self,
        content: &ContentRef,
        parsed: &ParsedRelease,
    ) -> std::result::Result<Vec<ContentMatch>, BoxError> {
        let module = self
            .module_for(content)
            .map_err(|e| Box::new(e) as BoxError)?;
        module.match_release(parsed).await
    }

    /// Pick the match that belongs to the content node under consideration, at a
    /// usable confidence. The module already drops force-fits below
    /// [`cellarr_media::AMBIGUOUS_CONFIDENCE`]; here we additionally require the
    /// match to actually be *for the node we are running* (the same library
    /// node), so a fanned-out multi-episode match cannot satisfy the wrong node.
    fn best_match(&self, content: &ContentRef, matches: Vec<ContentMatch>) -> Option<ContentMatch> {
        matches
            .into_iter()
            .filter(|m| {
                m.content_ref.id == content.id
                    && m.confidence.value() > cellarr_media::AMBIGUOUS_CONFIDENCE
            })
            .max_by(|a, b| {
                a.confidence
                    .value()
                    .partial_cmp(&b.confidence.value())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// Persist the resolved content metadata (year/overview/runtime/air-date) and
    /// artwork for a node that just confidently identified, via the configured
    /// resolver. Best-effort: with no resolver configured this is a no-op, and a
    /// resolver or persistence failure is logged and swallowed — metadata
    /// persistence must never block or fail acquisition (the library-safety rule
    /// scopes to *files*, not to enrichment).
    async fn persist_metadata(&self, matched: &ContentRef) {
        let Some(resolver) = self.metadata_resolver.as_ref() else {
            return;
        };
        let resolved = match resolver.resolve(matched).await {
            Ok(Some(r)) => r,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(content = %matched.id, error = %e, "metadata resolve failed; skipping persist");
                return;
            }
        };
        if resolved.meta.is_empty() {
            return;
        }
        use cellarr_core::repo::ContentRepository;
        if let Err(e) = self
            .db
            .content()
            .set_metadata(matched.id, &resolved.meta)
            .await
        {
            tracing::warn!(content = %matched.id, error = %e, "persisting content metadata failed");
        }
    }

    fn decide(
        &self,
        content_ref: &ContentRef,
        release: &Release,
        parsed: &ParsedRelease,
        on_disk: Option<OnDiskFile>,
    ) -> Result<Decision> {
        let ctx = DecisionContext {
            profile: &self.config.profile,
            custom_formats: &self.config.custom_formats,
            ranking: &self.config.ranking,
            blocklisted: false,
            proper_repack_policy: self.config.proper_repack_policy,
        };
        decide(content_ref.clone(), release, parsed, on_disk, &ctx)
            .map_err(|e| JobError::stage(Stage::Decide, e))
    }

    /// The currently-best on-disk file for `content`, expressed for the decision
    /// engine. Reads through the real media-file repository.
    async fn on_disk_for(&self, content: &ContentRef) -> Result<Option<OnDiskFile>> {
        use cellarr_core::repo::MediaFileRepository;
        let files = self
            .db
            .media_files()
            .list_for_content(content.id)
            .await
            .map_err(|e| JobError::Persistence(Box::new(e)))?;
        Ok(files
            .into_iter()
            .map(|f| OnDiskFile {
                file_id: f.id,
                quality_rank: f.quality.rank,
                custom_format_score: f.custom_format_score.unwrap_or(0),
                // Read the PERSISTED release type back from the media_file row so
                // the reconcile/upgrade decision never re-parses the title (the
                // season-pack re-grab-loop fix).
                release_type: f.release_type,
            })
            .max_by_key(|d| (d.quality_rank, d.custom_format_score)))
    }

    /// Persist a `media_file` row per imported destination and link each to the
    /// content node, recording the durable `release_type` and the quality the
    /// grab was graded on.
    ///
    /// This writes the authoritative on-disk state the reconcile/upgrade decision
    /// later reads through [`on_disk_for`](Self::on_disk_for) — so the type is
    /// read back, never re-parsed. Without it the imported file is invisible to
    /// the next cycle, which re-discovers the same release and re-grabs it forever
    /// (the loop this whole field exists to prevent).
    async fn persist_imported_files(
        &self,
        matched_ref: &ContentRef,
        title_parsed: &ParsedRelease,
        release_type: cellarr_core::ReleaseType,
        destinations: &[String],
    ) -> Result<()> {
        use cellarr_core::repo::MediaFileRepository;
        // The quality the decision engine graded the grab on, single-sourced from
        // core's catalogue. A parse it cannot bucket lands on the Unknown
        // sentinel, which still records the file as present (so the node is no
        // longer "missing") without claiming a quality it does not have.
        let quality = cellarr_core::resolve_quality(title_parsed, &self.config.ranking);
        // Distinct destination paths only: several planned moves can render to the
        // same on-disk path (e.g. a season pack whose per-episode naming is not
        // yet wired), and `media_file.path` is unique. One path is one file row.
        let mut seen_paths = std::collections::BTreeSet::new();
        for dest in destinations {
            if !seen_paths.insert(dest.clone()) {
                continue;
            }
            let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
            let file = cellarr_core::MediaFile {
                id: cellarr_core::MediaFileId::new(),
                path: dest.clone(),
                size,
                quality: quality.clone(),
                languages: title_parsed.languages.clone(),
                media_info: None,
                custom_format_score: None,
                release_type: Some(release_type),
            };
            self.db
                .media_files()
                .create(&file)
                .await
                .map_err(|e| JobError::Persistence(Box::new(e)))?;
            self.db
                .media_files()
                .link(matched_ref.id, file.id)
                .await
                .map_err(|e| JobError::Persistence(Box::new(e)))?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn grab_track_import(
        &self,
        run_id: PipelineRunId,
        stage: &mut Stage,
        content: &ContentRef,
        matched_ref: &ContentRef,
        release: &Release,
        parsed: &ParsedRelease,
        decision: Decision,
        _score: Score,
    ) -> Result<GrabTrackResult> {
        // --- Grab: persist the grab, hand to the download client ----------
        // Derive the durable release type from the title parse ONCE, here, and
        // persist it on the grab. Everything downstream (media_file, history,
        // reconcile) reads this back instead of re-parsing the title.
        let release_type = cellarr_core::ReleaseType::from_parsed(parsed);
        // Whether this grab replaces an existing file (an upgrade) decides which
        // notification event the eventual import fires (`Upgrade` vs `Import`).
        // Captured before `decision` is moved into the Grab transition below.
        let is_upgrade = matches!(decision.verdict, Verdict::Upgrade { .. });
        // The quality name the grab was graded on, surfaced to text notifications.
        let grab_quality = cellarr_core::resolve_quality(parsed, &self.config.ranking)
            .name
            .clone();
        let request = GrabRequest {
            content_ref: matched_ref.clone(),
            release: release.clone(),
            indexer_id: self.config.indexer_id,
            client_id: self.config.client_id,
            category: self.config.category.clone(),
            release_type: Some(release_type),
        };
        let grab_id = self
            .db
            .grabs()
            .create(&request)
            .await
            .map_err(|e| JobError::Persistence(Box::new(e)))?;

        // Decide -> Grab advance carries the grab verdict (the "why we grabbed").
        *stage = self
            .advance_with_decision(run_id, *stage, Some(decision))
            .await?; // -> Grab

        let download_id = match self.client.add(&request).await {
            Ok(id) => id,
            Err(e) => {
                // The client never accepted the grab, so there is nothing to
                // remove from it. Blocklist the release so grab-next moves past it.
                return self
                    .fail_grab(
                        run_id,
                        content.id,
                        grab_id,
                        None,
                        format!("grab failed: {e}"),
                        release,
                    )
                    .await;
            }
        };
        self.set_grab_status(grab_id, GrabStatus::Sent).await?;
        self.set_download_id(grab_id, &download_id).await?;
        self.append_history(
            run_id,
            content.id,
            HistoryEvent::Grabbed {
                grab_id,
                release_type: Some(release_type),
            },
        )
        .await?;

        // Fire the Connect `Grab` webhook (eventType: Grab) carrying the subject
        // + the grabbed release object Bazarr-push/Notifiarr read, then the same
        // event to the broadened notification providers.
        self.fire_grab_webhook(matched_ref, release).await;
        self.fire_provider_grab(matched_ref, release, &grab_quality)
            .await;

        *stage = self.advance(run_id, *stage, None).await?; // -> Track

        // --- Track: poll to completion, terminal failure, or stall --------
        let content_path = match self.track(&download_id).await {
            TrackOutcome::Completed(Some(path)) => {
                // Shared remote-path remapping: the client reports the path from
                // its own vantage point; rewrite it to where cellarr can see it
                // before Import. Applied here, once, for every download client.
                cellarr_core::apply_remote_path_mappings(
                    &self.config.remote_path_mappings,
                    &self.config.client_host,
                    &path,
                )
            }
            TrackOutcome::Completed(None) => {
                // Completed but the client reported no importable path: treat as a
                // failure, blocklist + remove, and grab the next release.
                return self
                    .fail_grab(
                        run_id,
                        content.id,
                        grab_id,
                        Some(&download_id),
                        "download completed without a content path".into(),
                        release,
                    )
                    .await;
            }
            // A hard failure, a sustained stall, or exhausted polls: blocklist the
            // release, remove the dead download (with its local data) from the
            // client, and signal grab-next.
            TrackOutcome::Failed(detail) => {
                return self
                    .fail_grab(
                        run_id,
                        content.id,
                        grab_id,
                        Some(&download_id),
                        detail,
                        release,
                    )
                    .await;
            }
        };
        self.set_grab_status(grab_id, GrabStatus::Completed).await?;
        self.append_history(
            run_id,
            content.id,
            HistoryEvent::DownloadCompleted { grab_id },
        )
        .await?;

        *stage = self.advance(run_id, *stage, None).await?; // -> Import

        // --- Import: stage -> verify -> commit -> log (cellarr-fs) --------
        match self
            .import(grab_id, matched_ref, parsed, &content_path)
            .await
        {
            Ok(destinations) => {
                self.set_grab_status(grab_id, GrabStatus::Imported).await?;
                // Persist a media_file row for each imported destination, carrying
                // the durable release type, and link it to the content node. This
                // is what makes the import "stick" in the authoritative state: the
                // next reconcile cycle reads these rows (with their persisted
                // release type + quality) via `on_disk_for` and the decision
                // recognizes an already-held full-season pack — closing the
                // re-grab loop. Computed from the title parse (the quality the
                // decision already graded the grab on).
                self.persist_imported_files(matched_ref, parsed, release_type, &destinations)
                    .await?;
                // The `Download` (import) webhook fires on the Import->Rename
                // advance; the `Rename` webhook fires on Rename->Notify. Both
                // carry the destination files the receiver reads. (Sonarr/Radarr
                // name the import event `Download`, kept for compatibility.)
                self.fire_files_webhook(WebhookEventType::Download, matched_ref, &destinations)
                    .await;
                // The provider notification: an `Upgrade` when this grab replaced
                // a lower-quality file, else an `Import`. This is also what a
                // media-server rescan provider keys on (`changes_library`).
                let import_event = if is_upgrade {
                    NotificationEvent::Upgrade
                } else {
                    NotificationEvent::Import
                };
                self.fire_provider_files(
                    import_event,
                    matched_ref,
                    release,
                    &grab_quality,
                    &destinations,
                )
                .await;
                *stage = self.advance(run_id, *stage, None).await?; // -> Rename
                self.fire_files_webhook(WebhookEventType::Rename, matched_ref, &destinations)
                    .await;
                *stage = self.advance(run_id, *stage, None).await?; // -> Notify
                self.append_history(run_id, content.id, HistoryEvent::Imported { grab_id })
                    .await?;
                self.advance(run_id, *stage, Some("imported".into()))
                    .await?; // -> Done
                Ok(GrabTrackResult::Done(RunOutcome::Imported {
                    grab_id,
                    destinations,
                }))
            }
            Err(detail) => {
                // import-failed -> hold for review (never silently drop, never
                // force-fit a destructive write). A held import is NOT a grab-next
                // trigger: the bytes are on disk and need a human, not another
                // grab.
                self.log(
                    run_id,
                    Stage::Import,
                    Stage::HeldForReview,
                    TransitionKind::Hold,
                    None,
                    Some(detail.clone()),
                )
                .await?;
                self.append_history(
                    run_id,
                    content.id,
                    HistoryEvent::HeldForReview {
                        reason: detail.clone(),
                    },
                )
                .await?;
                Ok(GrabTrackResult::Done(RunOutcome::HeldForReview {
                    reason: detail,
                }))
            }
        }
    }

    /// Poll the download client to a terminal point: completion, a hard failure,
    /// a sustained **stall**, or exhausted polls.
    ///
    /// Three terminal signals end tracking:
    /// - [`DownloadState::Completed`] → [`TrackOutcome::Completed`] with the
    ///   client-reported content path.
    /// - [`DownloadState::Failed`] → [`TrackOutcome::Failed`], surfacing the
    ///   client's `error_string` when present (qBittorrent `error`, SAB `Failed`).
    /// - A **stall**: the download is still in flight but has made no forward
    ///   progress *and* the client reports zero connected peers for
    ///   [`STALL_MAX_STAGNANT_POLLS`] consecutive cycles. A torrent with no peers
    ///   and no progress will never complete; failing it lets self-heal grab the
    ///   next release rather than waiting out the full poll budget. Progress (or a
    ///   peer appearing) resets the counter; an *unknown* peer count (`None`, e.g.
    ///   Usenet) never trips the stall.
    ///
    /// Bounded by `max_track_polls`; the logical clock advances between polls so
    /// tests never sleep.
    async fn track(&self, download_id: &str) -> TrackOutcome {
        let mut last_progress = f32::NEG_INFINITY;
        let mut stagnant_no_peer_polls: u32 = 0;
        for _ in 0..self.config.max_track_polls {
            let status = match self.client.status(download_id).await {
                Ok(s) => s,
                Err(e) => return TrackOutcome::Failed(format!("status poll failed: {e}")),
            };
            match status.state {
                DownloadState::Completed => return TrackOutcome::Completed(status.content_path),
                DownloadState::Failed => {
                    let detail = status
                        .error_string
                        .filter(|s| !s.trim().is_empty())
                        .map_or_else(
                            || "download failed".to_string(),
                            |e| format!("download failed: {e}"),
                        );
                    return TrackOutcome::Failed(detail);
                }
                DownloadState::Queued | DownloadState::Downloading => {
                    // Stall accounting: a poll counts toward a stall only when the
                    // client *reports* zero peers AND progress did not advance.
                    let advanced = status.progress > last_progress;
                    let no_peers = status.peers == Some(0);
                    let near_complete = status.progress >= NEAR_COMPLETE_PROGRESS;
                    if !advanced && no_peers && !near_complete {
                        stagnant_no_peer_polls += 1;
                        if stagnant_no_peer_polls >= STALL_MAX_STAGNANT_POLLS {
                            return TrackOutcome::Failed(format!(
                                "download stalled: no progress and no peers for \
                                 {STALL_MAX_STAGNANT_POLLS} polls (progress {:.0}%)",
                                status.progress * 100.0
                            ));
                        }
                    } else {
                        stagnant_no_peer_polls = 0;
                    }
                    last_progress = last_progress.max(status.progress);
                    // Event-driven progress is preferred (docs/03-pipeline.md);
                    // absent a webhook, poll with the (logical) clock advancing.
                    let _ = self.clock.now_secs();
                    // Wait between polls so the poll budget spans a real,
                    // multi-minute download. Zero in tests (no real sleep); a
                    // few seconds in the live daemon.
                    if self.config.track_poll_interval.is_zero() {
                        tokio::task::yield_now().await;
                    } else {
                        tokio::time::sleep(self.config.track_poll_interval).await;
                    }
                }
            }
        }
        TrackOutcome::Failed("tracking timed out".into())
    }

    /// Build and execute the import plan for one completed download.
    ///
    /// Re-parses the *file* path (the second parse, the source of truth) and
    /// asks the media module for naming tokens, renders the destination via
    /// cellarr-fs, then runs the crash-safe `plan_import`/`execute_import`.
    async fn import(
        &self,
        grab_id: GrabId,
        matched_ref: &ContentRef,
        title_parsed: &ParsedRelease,
        content_path: &str,
    ) -> std::result::Result<Vec<String>, String> {
        let src = std::path::Path::new(content_path);
        if !src.exists() {
            return Err(format!(
                "download content path does not exist: {content_path}"
            ));
        }
        // Collect the source file(s). A single-file download is the file itself;
        // a directory is walked for its media files.
        let sources = collect_sources(src).map_err(|e| format!("scan source: {e}"))?;
        if sources.is_empty() {
            return Err(format!("no importable files under {content_path}"));
        }

        // The second parse: re-parse the actual file name and verify it does not
        // disagree with the grab's intent beyond tolerance. Here the tolerance
        // check is a coarse coordinates agreement; a richer media-info probe is
        // cellarr-fs/cellarr-media's remit.
        let module = self
            .module_for(matched_ref)
            .map_err(|e| format!("module: {e}"))?;
        let tokens = module
            .naming_tokens(matched_ref)
            .await
            .map_err(|e| format!("naming tokens: {e}"))?;

        let mut moves = Vec::with_capacity(sources.len());
        for source in &sources {
            let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("mkv");
            let rel =
                cellarr_fs::render_name(&self.config.naming_format, &with_ext_token(&tokens, ext))
                    .map_err(|e| format!("render name: {e}"))?;
            let dest = self.config.library_root.join(&rel);
            moves.push(PlannedMove {
                source_path: source.to_string_lossy().into_owned(),
                destination_path: dest.to_string_lossy().into_owned(),
                content_ids: vec![matched_ref.id],
                replaces: None,
                replaced_path: None,
                hardlink: false,
            });
        }

        // Verify the second parse agrees with the grab intent on coordinates,
        // when both carry them. A hard disagreement holds, per the pipeline doc.
        verify_second_parse(title_parsed, &sources)?;

        let plan = cellarr_fs::plan_import(grab_id, moves)
            .await
            .map_err(|e| format!("plan import: {e}"))?;
        let result = cellarr_fs::execute_import(&plan)
            .await
            .map_err(|e| format!("execute import: {e}"))?;
        let destinations: Vec<String> = result
            .moves
            .into_iter()
            .map(|m| m.destination_path.to_string_lossy().into_owned())
            .collect();

        // Best-effort, post-commit metadata sidecars (Kodi/Jellyfin `.nfo`). The
        // media is already durable; a sidecar failure is logged and never fails
        // the import (so the crash-safe stage->verify->commit guarantee stands).
        if self.config.write_nfo {
            self.write_nfo_sidecars(matched_ref, &destinations).await;
        }

        Ok(destinations)
    }

    /// Write the `.nfo` metadata sidecars next to the imported destinations, using
    /// the node's persisted content metadata. For a movie, one `movie.nfo`; for a
    /// TV episode, one `<file>.nfo` per episode file plus a `tvshow.nfo` in the
    /// series root. Entirely best-effort: any error is logged and swallowed.
    async fn write_nfo_sidecars(&self, matched_ref: &ContentRef, destinations: &[String]) {
        use cellarr_core::Coordinates;
        let meta = self
            .db
            .content()
            .metadata(matched_ref.id)
            .await
            .unwrap_or(None)
            .unwrap_or_default();
        let (season, episode) = match &matched_ref.coords {
            Coordinates::Episode {
                season, episode, ..
            } => (Some(*season), Some(*episode)),
            _ => (None, None),
        };
        let nfo = cellarr_fs::NfoMetadata {
            title: meta.title.clone(),
            year: meta.year,
            overview: meta.overview.clone(),
            runtime: meta.runtime,
            air_date: meta.air_date.clone(),
            season,
            episode,
        };
        let (file_kind, write_show) = match matched_ref.media_type {
            cellarr_core::MediaType::Tv => (cellarr_fs::NfoKind::Episode, true),
            _ => (cellarr_fs::NfoKind::Movie, false),
        };
        let mut wrote_show = false;
        for dest in destinations {
            let path = std::path::Path::new(dest);
            if let Err(e) = cellarr_fs::write_sidecar(file_kind, path, &nfo).await {
                tracing::warn!(dest = %dest, error = %e, "writing .nfo sidecar failed");
            }
            // The series-level tvshow.nfo lives in the series root. The episode
            // file path is `<root>/.../Season NN/Episode.ext`; placing tvshow.nfo
            // beside the episode keeps it discoverable without needing the root
            // path here, and is what Kodi finds when scanning the show folder.
            if write_show && !wrote_show {
                if let Err(e) =
                    cellarr_fs::write_sidecar(cellarr_fs::NfoKind::Series, path, &nfo).await
                {
                    tracing::warn!(dest = %dest, error = %e, "writing tvshow.nfo failed");
                }
                wrote_show = true;
            }
        }
    }

    // --- Failure / transition / log helpers ------------------------------

    /// Handle a failed/stalled grab: mark it Blocklisted, record the release in the
    /// blocklist, remove the dead download from the client (with its local data),
    /// log the failure transition + history, and signal grab-next.
    ///
    /// `download_id` is `Some` once the client accepted the grab (so there is a
    /// download to remove) and `None` when the client never accepted it (nothing
    /// to remove). Removal is best-effort: a client that errors on remove (or has
    /// already dropped the download) must not abort the self-heal — the release is
    /// still blocklisted and the next-best is still grabbed.
    async fn fail_grab(
        &self,
        run_id: PipelineRunId,
        content_id: cellarr_core::ContentId,
        grab_id: GrabId,
        download_id: Option<&str>,
        detail: String,
        release: &Release,
    ) -> Result<GrabTrackResult> {
        // Mark the grab Blocklisted so even outside this run a re-search never
        // re-grabs the same dead release.
        self.set_grab_status(grab_id, GrabStatus::Blocklisted)
            .await?;
        self.blocklist_release(content_id, release, &detail).await?;

        // Remove the dead download from the client, deleting its local data, so a
        // stalled/failed download is not left consuming disk or a slot. Best-effort.
        if let Some(download_id) = download_id {
            if let Err(e) = self
                .client
                .remove(download_id, /* delete_data = */ true)
                .await
            {
                tracing::warn!(
                    %download_id,
                    error = %e,
                    "removing failed download from client failed; continuing self-heal"
                );
            }
        }

        self.log(
            run_id,
            Stage::Grab,
            Stage::Failed,
            TransitionKind::Fail,
            None,
            Some(detail.clone()),
        )
        .await?;
        self.append_history(
            run_id,
            content_id,
            HistoryEvent::DownloadFailed {
                grab_id,
                detail: detail.clone(),
            },
        )
        .await?;
        Ok(GrabTrackResult::FailedGrabNext(detail))
    }

    /// Add a failed release to the blocklist (idempotent on content+release key).
    async fn blocklist_release(
        &self,
        content_id: cellarr_core::ContentId,
        release: &Release,
        reason: &str,
    ) -> Result<()> {
        let entry =
            BlocklistEntry::from_release(content_id, release, reason.to_string(), self.now());
        BlocklistRepository::add(&self.db.blocklist(), &entry)
            .await
            .map_err(|e| JobError::Persistence(Box::new(e)))
    }

    /// Whether `release` is already blocklisted for `content_id`.
    async fn is_blocklisted(
        &self,
        content_id: cellarr_core::ContentId,
        release: &Release,
    ) -> Result<bool> {
        BlocklistRepository::is_blocklisted(&self.db.blocklist(), content_id, release)
            .await
            .map_err(|e| JobError::Persistence(Box::new(e)))
    }

    async fn advance(
        &self,
        run_id: PipelineRunId,
        from: Stage,
        note: Option<String>,
    ) -> Result<Stage> {
        self.advance_with_decision_note(run_id, from, None, note)
            .await
    }

    async fn advance_with_decision(
        &self,
        run_id: PipelineRunId,
        from: Stage,
        decision: Option<Decision>,
    ) -> Result<Stage> {
        self.advance_with_decision_note(run_id, from, decision, None)
            .await
    }

    async fn advance_with_decision_note(
        &self,
        run_id: PipelineRunId,
        from: Stage,
        decision: Option<Decision>,
        note: Option<String>,
    ) -> Result<Stage> {
        let transition = Transition::advance(from)?;
        let record = DecisionLogRecord {
            at: self.now(),
            run_id,
            transition,
            decision,
            note,
        };
        DecisionLogRepository::append(&self.db.decision_log(), &record)
            .await
            .map_err(|e| JobError::Persistence(Box::new(e)))?;
        Ok(transition.to)
    }

    #[allow(clippy::too_many_arguments)]
    async fn log(
        &self,
        run_id: PipelineRunId,
        from: Stage,
        to: Stage,
        kind: TransitionKind,
        decision: Option<Decision>,
        note: Option<String>,
    ) -> Result<()> {
        let transition = Transition::new(from, to, kind)?;
        let record = DecisionLogRecord {
            at: self.now(),
            run_id,
            transition,
            decision,
            note,
        };
        DecisionLogRepository::append(&self.db.decision_log(), &record)
            .await
            .map_err(|e| JobError::Persistence(Box::new(e)))
    }

    #[allow(clippy::too_many_arguments)]
    async fn log_decision(
        &self,
        run_id: PipelineRunId,
        from: Stage,
        to: Stage,
        kind: TransitionKind,
        decision: Decision,
        note: Option<String>,
    ) -> Result<()> {
        self.log(run_id, from, to, kind, Some(decision), note).await
    }

    async fn append_history(
        &self,
        run_id: PipelineRunId,
        content_id: cellarr_core::ContentId,
        event: HistoryEvent,
    ) -> Result<()> {
        let record = HistoryRecord {
            at: self.now(),
            content_id,
            run_id,
            event,
        };
        HistoryRepository::append(&self.db.history(), &record)
            .await
            .map_err(|e| JobError::Persistence(Box::new(e)))
    }

    async fn set_grab_status(&self, id: GrabId, status: GrabStatus) -> Result<()> {
        GrabRepository::set_status(&self.db.grabs(), id, status)
            .await
            .map_err(|e| JobError::Persistence(Box::new(e)))
    }

    async fn set_download_id(&self, id: GrabId, download_id: &str) -> Result<()> {
        GrabRepository::set_download_id(&self.db.grabs(), id, download_id)
            .await
            .map_err(|e| JobError::Persistence(Box::new(e)))
    }

    // --- Connect-webhook firing ------------------------------------------

    /// Fire the `Grab` webhook for a grabbed release. Best-effort: no notifier
    /// configured (the offline path) sends nothing; a delivery failure is logged
    /// inside the dispatcher and never affects the run.
    async fn fire_grab_webhook(&self, content_ref: &ContentRef, release: &Release) {
        let Some(notifier) = self.notifier.as_ref() else {
            return;
        };
        let subject = self.subject_for(content_ref).await;
        let payload = WebhookPayload::for_subject(
            WebhookEventType::Grab,
            content_ref.media_type,
            subject,
            String::new(),
        )
        .with_release(cellarr_core::WebhookRelease::from_release(release, None));
        notifier.dispatch(payload).await;
    }

    /// Fire a `Download`(import) or `Rename` webhook carrying the destination
    /// files. Best-effort, like [`fire_grab_webhook`](Self::fire_grab_webhook).
    async fn fire_files_webhook(
        &self,
        event_type: WebhookEventType,
        content_ref: &ContentRef,
        destinations: &[String],
    ) {
        let Some(notifier) = self.notifier.as_ref() else {
            return;
        };
        let subject = self.subject_for(content_ref).await;
        let files: Vec<WebhookFile> = destinations
            .iter()
            .map(|d| WebhookFile {
                path: d.clone(),
                previous_path: None,
            })
            .collect();
        let payload =
            WebhookPayload::for_subject(event_type, content_ref.media_type, subject, String::new())
                .with_files(files);
        notifier.dispatch(payload).await;
    }

    /// Fire the `Grab` provider notification (Discord/Telegram/Email/Custom
    /// Script/etc.) carrying the subject + grabbed release. Best-effort; no
    /// provider notifier configured (the offline path) sends nothing.
    async fn fire_provider_grab(&self, content_ref: &ContentRef, release: &Release, quality: &str) {
        let Some(notifier) = self.provider_notifier.as_ref() else {
            return;
        };
        let subject = self.notification_subject_for(content_ref).await;
        let message = NotificationMessage::new(NotificationEvent::Grab, String::new())
            .with_subject(subject)
            .with_release(NotificationRelease::from_release(
                release,
                quality_opt(quality),
            ));
        notifier.dispatch(message).await;
    }

    /// Fire the `Import`/`Upgrade` provider notification carrying the destination
    /// files (also what a media-server rescan provider acts on). Best-effort.
    async fn fire_provider_files(
        &self,
        event: NotificationEvent,
        content_ref: &ContentRef,
        release: &Release,
        quality: &str,
        destinations: &[String],
    ) {
        let Some(notifier) = self.provider_notifier.as_ref() else {
            return;
        };
        let subject = self.notification_subject_for(content_ref).await;
        let message = NotificationMessage::new(event, String::new())
            .with_subject(subject)
            .with_release(NotificationRelease::from_release(
                release,
                quality_opt(quality),
            ))
            .with_files(destinations.to_vec());
        notifier.dispatch(message).await;
    }

    /// Build the provider-notification subject for a content node — its id +
    /// best-known title + media type (so a provider can label TV vs movie).
    async fn notification_subject_for(&self, content_ref: &ContentRef) -> NotificationSubject {
        let title = self
            .db
            .content()
            .title_for(content_ref.id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| content_ref.id.to_string());
        NotificationSubject {
            id: content_ref.id.to_string(),
            title,
            year: None,
            media_type: Some(content_ref.media_type),
        }
    }

    /// Build the webhook subject for a content node — its id + best-known title.
    async fn subject_for(&self, content_ref: &ContentRef) -> WebhookSubject {
        let title = self
            .db
            .content()
            .title_for(content_ref.id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| content_ref.id.to_string());
        // External ids (tvdbId/tmdbId/imdbId) live behind the node's title_id in
        // cellarr-meta; resolving them here is a documented follow-up (the same
        // gap the v3 list resources carry). The subject still carries the id +
        // title every receiver keys on, and which-field-is-present (series vs
        // movie) conveys the media type.
        WebhookSubject {
            id: content_ref.id.to_string(),
            title,
            year: None,
            tvdb_id: None,
            tmdb_id: None,
            imdb_id: None,
        }
    }

    fn module_for(&self, content: &ContentRef) -> Result<&dyn cellarr_media::DynMediaModule> {
        self.registry
            .get(content.media_type)
            .ok_or(JobError::NotConfigured {
                resource: "media module",
                detail: format!("{:?}", content.media_type),
            })
    }

    /// The current time, sourced from the injected clock so logs are
    /// deterministic in tests. The clock yields seconds; we build an
    /// `OffsetDateTime` from that so the persisted RFC3339 timestamps are stable.
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(self.clock.now_secs() as i64)
            .unwrap_or_else(|_| OffsetDateTime::now_utc())
    }

    // --- manual import (loose-folder scan + commit) ----------------------

    /// **Manual-import scan**: walk `folder` for media files and, for each, parse
    /// the name, attempt to identify it onto a content node, and report a
    /// [`ManualImportCandidate`]. This is **read-only** — it stats and parses, but
    /// moves, renames, and deletes nothing (the user reviews the candidates before
    /// committing through [`import_manual`](Self::import_manual)).
    ///
    /// The walk reuses the same [`cellarr_fs::scan`] inventory the migration
    /// "recognize in place" path uses (so the folder is enumerated identically),
    /// then runs the same Parse→Identify steps a pipeline run performs (real
    /// `cellarr-parse` + the media module's `match_release`). A file the parser
    /// cannot bucket, or that does not confidently identify to a single node, is
    /// still returned — with its `suggested` left `None` and a `rejections` entry —
    /// so the screen can show it and let the user map it by hand.
    ///
    /// # Errors
    /// Returns [`JobError`] only for an infrastructure failure (the folder could
    /// not be scanned). A per-file parse/identify miss is **not** an error — it is
    /// a candidate carrying a rejection.
    pub async fn scan_manual_import(
        &self,
        folder: &std::path::Path,
    ) -> Result<Vec<ManualImportCandidate>> {
        let inventory = cellarr_fs::scan(folder.to_path_buf())
            .await
            .map_err(|e| JobError::stage(Stage::Import, e))?;

        let mut out = Vec::with_capacity(inventory.entries.len());
        for entry in &inventory.entries {
            let name = entry
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let parsed = cellarr_parse::parse_title(&name);
            let quality = cellarr_core::resolve_quality(&parsed, &self.config.ranking);

            let (suggested, rejections) = self.suggest_placement(&parsed).await;
            out.push(ManualImportCandidate {
                path: entry.path.to_string_lossy().into_owned(),
                name,
                size: entry.size,
                parsed_title: parsed.clean_title.clone(),
                quality,
                suggested,
                rejections,
            });
        }
        Ok(out)
    }

    /// Identify a parsed loose file onto a content node, returning the suggested
    /// placement or the reason no confident, unambiguous match was found.
    ///
    /// Tries every registered media module's `match_release` (a loose folder may
    /// mix movies and TV), keeps the matches above [`cellarr_media::AMBIGUOUS_CONFIDENCE`],
    /// and suggests the single most-confident one. Zero confident matches, or a tie
    /// the identifier already flagged ambiguous, yields `None` + a rejection so the
    /// user maps it by hand — never a guessed placement (the library-safety rule).
    async fn suggest_placement(
        &self,
        parsed: &ParsedRelease,
    ) -> (Option<ManualImportSuggestion>, Vec<String>) {
        use cellarr_core::Coordinates;

        let mut best: Option<ContentMatch> = None;
        for media_type in self.registry.media_types() {
            let Some(module) = self.registry.get(media_type) else {
                continue;
            };
            let matches = match module.match_release(parsed).await {
                Ok(m) => m,
                Err(_) => continue,
            };
            for m in matches {
                if m.confidence.value() <= cellarr_media::AMBIGUOUS_CONFIDENCE {
                    continue;
                }
                let better = best
                    .as_ref()
                    .is_none_or(|b| m.confidence.value() > b.confidence.value());
                if better {
                    best = Some(m);
                }
            }
        }

        let Some(matched) = best else {
            return (
                None,
                vec!["could not confidently identify this file to a library item".to_string()],
            );
        };
        let (season, episode) = match &matched.content_ref.coords {
            Coordinates::Episode {
                season, episode, ..
            } => (Some(*season), Some(*episode)),
            _ => (None, None),
        };
        (
            Some(ManualImportSuggestion {
                content_id: matched.content_ref.id,
                season,
                episode,
            }),
            Vec::new(),
        )
    }

    /// **Manual-import commit**: import the user's chosen loose files onto the
    /// content nodes they picked, each through the **same crash-safe
    /// stage→verify→commit→log import path** an automatic run uses
    /// ([`cellarr_fs::plan_import`] / [`cellarr_fs::execute_import`]).
    ///
    /// For each request: resolve the chosen content node, re-parse the *file* name
    /// (the second parse — the source of truth), render the destination via the
    /// media module's naming tokens + the configured naming format, and run the
    /// crash-safe planner/executor. On success the imported file is persisted as a
    /// `media_file` row linked to the node (so the library recognizes it and the
    /// node is no longer "missing"), exactly as the automatic import does. A file
    /// is **never moved until its plan is verified**, and the old library file (on
    /// an overwrite) is never removed before the new one is durable — the
    /// library-safety guarantee is the import path's, untouched here.
    ///
    /// Per-request failures are collected and returned as `Err` strings rather than
    /// aborting the whole batch, so one un-importable file does not strand the rest.
    ///
    /// # Errors
    /// Returns [`JobError`] only for an infrastructure failure (a repository write
    /// failed). A per-file domain failure (node not found, plan/verify failed) is
    /// carried in the returned `errors` vector, not errored.
    pub async fn import_manual(
        &self,
        requests: &[ManualImportRequest],
    ) -> Result<(Vec<ManualImportResult>, Vec<String>)> {
        use cellarr_core::repo::ContentRepository;

        let mut imported = Vec::new();
        let mut errors = Vec::new();
        for req in requests {
            let node = match ContentRepository::get(&self.db.content(), req.content_id).await {
                Ok(Some(n)) => n,
                Ok(None) => {
                    errors.push(format!("content {} not found", req.content_id));
                    continue;
                }
                Err(e) => return Err(JobError::Persistence(Box::new(e))),
            };
            match self.import_one_manual(&node, &req.path).await {
                Ok(result) => imported.push(result),
                Err(detail) => errors.push(format!("{}: {detail}", req.path)),
            }
        }
        Ok((imported, errors))
    }

    /// Import one chosen loose file onto `matched_ref` through the crash-safe path,
    /// persisting the resulting media-file row. Mirrors [`import`](Self::import) but
    /// drives a single user-chosen source rather than a download directory, and is
    /// scoped to one destination node.
    async fn import_one_manual(
        &self,
        matched_ref: &ContentRef,
        source_path: &str,
    ) -> std::result::Result<ManualImportResult, String> {
        let src = std::path::Path::new(source_path);
        if !src.is_file() {
            return Err(format!("source file does not exist: {source_path}"));
        }

        // The second parse: the on-disk file name is the source of truth. Render the
        // destination from the media module's naming tokens + the file extension.
        let file_parsed = cellarr_parse::parse_title(
            src.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
        );
        let module = self
            .module_for(matched_ref)
            .map_err(|e| format!("module: {e}"))?;
        let tokens = module
            .naming_tokens(matched_ref)
            .await
            .map_err(|e| format!("naming tokens: {e}"))?;
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("mkv");
        // The naming format must match the *matched node's* media type, not the
        // config's (a manual import scans one config across a mixed library set, so
        // the config's format — derived from whichever library sorted first — can be
        // the wrong shape for this node). Render against the node's media-type
        // format so a TV node renders with the series format even when a movie
        // library was picked to build the config, and vice-versa.
        let naming_format = naming_format_for(&self.config.naming_format, matched_ref.media_type);
        let rel = cellarr_fs::render_name(naming_format, &with_ext_token(&tokens, ext))
            .map_err(|e| format!("render name: {e}"))?;
        let dest = self.config.library_root.join(&rel);

        // Verify the file's parse does not contradict the chosen node's coordinates
        // (the same library-safety gate the automatic import applies), then plan and
        // commit through the crash-safe path. A grab id is minted to key the plan's
        // staging area; a manual import has no grab row, so the id is local to the plan.
        let intent = ParsedRelease {
            coordinates: vec![matched_ref.coords.clone()],
            ..ParsedRelease::new(source_path)
        };
        verify_second_parse(&intent, std::slice::from_ref(&src.to_path_buf()))?;

        let moves = vec![PlannedMove {
            source_path: source_path.to_string(),
            destination_path: dest.to_string_lossy().into_owned(),
            content_ids: vec![matched_ref.id],
            replaces: None,
            replaced_path: None,
            hardlink: false,
        }];
        let plan = cellarr_fs::plan_import(GrabId::new(), moves)
            .await
            .map_err(|e| format!("plan import: {e}"))?;
        let result = cellarr_fs::execute_import(&plan)
            .await
            .map_err(|e| format!("execute import: {e}"))?;
        let destination_path = result
            .moves
            .first()
            .map(|m| m.destination_path.to_string_lossy().into_owned())
            .unwrap_or_default();

        // Persist the media-file row + content link so the library recognizes the
        // import (the node is no longer "missing"), the same authoritative write the
        // automatic import makes — but idempotently: a manual re-commit of an
        // already-imported source lands at the same destination, and `media_file.path`
        // is unique, so we skip the create+link when the node already carries a file
        // at this path (re-issuing the commit must not error or duplicate the row). A
        // manual import carries no grab provenance, so the release type is left unknown.
        let already_linked = {
            use cellarr_core::repo::MediaFileRepository;
            self.db
                .media_files()
                .list_for_content(matched_ref.id)
                .await
                .map_err(|e| format!("reading existing media files: {e}"))?
                .iter()
                .any(|f| f.path == destination_path)
        };
        if !already_linked {
            self.persist_imported_files(
                matched_ref,
                &file_parsed,
                cellarr_core::ReleaseType::from_parsed(&file_parsed),
                std::slice::from_ref(&destination_path),
            )
            .await
            .map_err(|e| format!("persist imported file: {e}"))?;
        }

        Ok(ManualImportResult {
            source_path: source_path.to_string(),
            destination_path,
            content_id: matched_ref.id,
        })
    }
}

/// Map a resolved quality name to the `Option<String>` a notification carries:
/// the Unknown-sentinel/empty name becomes `None` (the parser could not bucket
/// the release), so a notification never claims a quality it does not have.
fn quality_opt(name: &str) -> Option<String> {
    if name.is_empty() || name.eq_ignore_ascii_case("unknown") {
        None
    } else {
        Some(name.to_string())
    }
}

/// Human text for a reject reason, for the decision-log note.
fn reason_text(reason: &cellarr_core::RejectReason) -> String {
    use cellarr_core::RejectReason as R;
    match reason {
        R::QualityNotAllowed => "quality not allowed by profile".into(),
        R::BelowMinimumCustomFormatScore => "below minimum custom-format score".into(),
        R::Blocklisted => "release is blocklisted".into(),
        R::SizeOutOfRange => "size out of configured range".into(),
        R::LanguageRequirementUnmet => "required language missing".into(),
        R::CutoffAlreadyMet => "cutoff already met".into(),
        R::NotAnUpgrade => "not an upgrade over existing file".into(),
        R::Other { detail } => detail.clone(),
    }
}

/// Choose the naming format to render a node of `media_type` with.
///
/// `configured` is the per-run format from [`RunnerConfig::naming_format`]. For an
/// automatic acquisition (one node, config built from that node's library) it
/// already targets the node's media type and is returned unchanged — user naming
/// customization is honored. For a *manual* import, one config is reused across a
/// mixed library set, so the configured format can be the wrong shape for this
/// node (a movie format applied to a TV node has no `{Series Title}` token and
/// would hard-error). When the configured format does not reference the node's
/// primary title token, fall back to the built-in default for the node's media
/// type so the import still lands with a correctly-shaped name.
fn naming_format_for(configured: &str, media_type: cellarr_core::MediaType) -> &str {
    use cellarr_core::MediaType;
    let primary_token = match media_type {
        MediaType::Movie => "{Movie Title}",
        MediaType::Tv => "{Series Title}",
        // Music/book formats key off the generic {Title} token.
        MediaType::Music | MediaType::Book => "{Title}",
    };
    if configured.contains(primary_token) {
        return configured;
    }
    default_naming_format(media_type)
}

/// The built-in per-media-type naming format — the safe default shape used when a
/// configured format does not fit the node's media type (see [`naming_format_for`]).
/// Mirrors the daemon's default so a node renders with its type's conventional
/// layout regardless of which library supplied the run config.
fn default_naming_format(media_type: cellarr_core::MediaType) -> &'static str {
    use cellarr_core::MediaType;
    match media_type {
        MediaType::Tv => {
            "{Series Title}/Season {Season}/{Series Title} - S{Season}E{Episode}.{Extension}"
        }
        MediaType::Movie => "{Movie Title} ({Release Year})/{Movie Title}.{Extension}",
        MediaType::Music | MediaType::Book => "{Title}.{Extension}",
    }
}

/// Append a synthetic `Extension` naming token so the rename format can preserve
/// the file extension without the media module having to know it.
fn with_ext_token(tokens: &NamingTokens, ext: &str) -> NamingTokens {
    let mut t = tokens.tokens.clone();
    t.push(("Extension".to_string(), ext.to_string()));
    NamingTokens { tokens: t }
}

/// Collect importable source files: the file itself, or every file under a
/// download directory (one level; download clients lay content out flat or in a
/// single folder).
fn collect_sources(src: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    if src.is_file() {
        return Ok(vec![src.to_path_buf()]);
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// The second-parse verification gate. Re-parse each source file name and confirm
/// it does not disagree with the title parse's coordinates beyond tolerance.
///
/// Tolerance here: if both the title parse and the file parse carry TV
/// coordinates, the season/episode must agree. Movies and missing coordinates
/// pass (a coarse but safe check; richer media-info verification is out of
/// scope for the runner). A disagreement returns `Err`, which the caller turns
/// into an import-held outcome — never a force-fit overwrite.
fn verify_second_parse(
    title_parsed: &ParsedRelease,
    sources: &[std::path::PathBuf],
) -> std::result::Result<(), String> {
    use cellarr_core::Coordinates;
    let title_coords: Vec<&Coordinates> = title_parsed.coordinates.iter().collect();
    if title_coords.is_empty() {
        return Ok(());
    }
    for source in sources {
        let name = source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let file_parsed = cellarr_parse::parse_title(name);
        if file_parsed.coordinates.is_empty() {
            // The file name carried no numbering (common for movies and for
            // bare-named files); nothing to contradict.
            continue;
        }
        let agree = file_parsed
            .coordinates
            .iter()
            .any(|fc| coords_agree(title_coords.as_slice(), fc));
        if !agree {
            return Err(format!(
                "file parse {:?} disagrees with grab intent {:?}",
                file_parsed.coordinates, title_parsed.coordinates
            ));
        }
    }
    Ok(())
}

/// Whether a file-parsed coordinate matches any of the title-parsed coordinates
/// on its identifying numbers (season+episode for TV; movies always agree).
fn coords_agree(
    title_coords: &[&cellarr_core::Coordinates],
    file: &cellarr_core::Coordinates,
) -> bool {
    use cellarr_core::Coordinates as Co;
    match file {
        Co::Movie => true,
        Co::Episode {
            season, episode, ..
        } => title_coords.iter().any(|tc| match tc {
            // A direct episode match (the file is the episode the title named).
            Co::Episode {
                season: ts,
                episode: te,
                ..
            } => ts == season && te == episode,
            // A season pack legitimately contains that season's episodes: a file
            // parsed as S02E01 does NOT contradict a grab whose intent was the
            // whole of season 2. We agree on the season (the unit the pack was
            // grabbed for) rather than holding every season-pack import for
            // review. The episode-level placement is handled downstream.
            Co::SeasonPack { season: ts } => u32::from(*ts) == *season,
            _ => false,
        }),
        other => title_coords.contains(&other),
    }
}

/// Make [`PipelineRunner`] usable behind an `Arc` of its seams (the scheduler
/// holds shared seams). A thin owned-handle variant.
pub type SharedRegistry = Arc<MediaRegistry>;

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::Coordinates as Co;

    // --- reason_text: every reject reason renders human text -----------------

    #[test]
    fn reason_text_covers_every_reject_reason() {
        use cellarr_core::RejectReason as R;
        // Each branch must produce non-empty, distinct text so the decision log is
        // readable. (A missing arm would fail to compile, but this also pins the
        // strings against accidental blanking.)
        let cases = [
            reason_text(&R::QualityNotAllowed),
            reason_text(&R::BelowMinimumCustomFormatScore),
            reason_text(&R::Blocklisted),
            reason_text(&R::SizeOutOfRange),
            reason_text(&R::LanguageRequirementUnmet),
            reason_text(&R::CutoffAlreadyMet),
            reason_text(&R::NotAnUpgrade),
        ];
        for text in &cases {
            assert!(!text.is_empty(), "reject reason text must not be empty");
        }
        // The free-form `Other` carries its own detail verbatim.
        assert_eq!(
            reason_text(&R::Other {
                detail: "custom detail".into()
            }),
            "custom detail"
        );
    }

    // --- with_ext_token: appends the extension without disturbing tokens ------

    #[test]
    fn with_ext_token_appends_extension_preserving_existing() {
        let base = NamingTokens {
            tokens: vec![
                ("Series Title".to_string(), "Show".to_string()),
                ("Season".to_string(), "1".to_string()),
            ],
        };
        let with_ext = with_ext_token(&base, "mkv");
        // The original tokens are preserved in order...
        assert_eq!(with_ext.tokens[0].0, "Series Title");
        assert_eq!(with_ext.tokens[1].0, "Season");
        // ...and the Extension token is appended last.
        let ext = with_ext.tokens.last().unwrap();
        assert_eq!(ext.0, "Extension");
        assert_eq!(ext.1, "mkv");
        // The source tokens are untouched (no extension leaked in).
        assert_eq!(base.tokens.len(), 2);
    }

    // --- naming_format_for: per-node media-type format selection -------------

    #[test]
    fn naming_format_for_keeps_matching_configured_format() {
        use cellarr_core::MediaType;
        // A configured movie format is honored for a movie node (user naming
        // customization is preserved when the shape fits the node).
        let movie_fmt = "{Movie Title} {{custom}} ({Release Year})/{Movie Title}.{Extension}";
        assert_eq!(
            naming_format_for(movie_fmt, MediaType::Movie),
            movie_fmt,
            "a movie format fits a movie node and is kept verbatim"
        );
        let tv_fmt = "{Series Title}/{Series Title} S{Season}E{Episode}.{Extension}";
        assert_eq!(naming_format_for(tv_fmt, MediaType::Tv), tv_fmt);
    }

    #[test]
    fn naming_format_for_falls_back_when_configured_format_mismatches_node() {
        use cellarr_core::MediaType;
        // The bug: a manual import builds one config from whichever library sorted
        // first. A TV node committed under a *movie* config must NOT render with the
        // movie format (it has no {Series Title} token); it falls back to the TV
        // default so the import lands.
        let movie_fmt = "{Movie Title} ({Release Year})/{Movie Title}.{Extension}";
        let tv_default = naming_format_for(movie_fmt, MediaType::Tv);
        assert!(tv_default.contains("{Series Title}"));
        assert!(tv_default.contains("{Episode}"));
        // And the reverse: a movie node under a TV config falls back to the movie
        // default.
        let tv_fmt =
            "{Series Title}/Season {Season}/{Series Title} - S{Season}E{Episode}.{Extension}";
        let movie_default = naming_format_for(tv_fmt, MediaType::Movie);
        assert!(movie_default.contains("{Movie Title}"));
        assert!(movie_default.contains("{Release Year}"));
    }

    // --- coords_agree: the library-safety second-parse agreement -------------

    fn ep(season: u32, episode: u32) -> Co {
        Co::Episode {
            season,
            episode,
            absolute: None,
        }
    }

    #[test]
    fn coords_agree_movie_always_agrees() {
        // A file parsed as a movie never contradicts any grab intent.
        let title = [ep(2, 5)];
        let refs: Vec<&Co> = title.iter().collect();
        assert!(coords_agree(&refs, &Co::Movie));
    }

    #[test]
    fn coords_agree_episode_matches_same_season_and_episode_only() {
        let title = [ep(2, 5)];
        let refs: Vec<&Co> = title.iter().collect();
        assert!(coords_agree(&refs, &ep(2, 5)), "exact match agrees");
        assert!(
            !coords_agree(&refs, &ep(2, 6)),
            "different episode must NOT agree"
        );
        assert!(
            !coords_agree(&refs, &ep(3, 5)),
            "different season must NOT agree"
        );
    }

    #[test]
    fn coords_agree_season_pack_intent_accepts_any_episode_of_that_season() {
        // A grab whose intent was the whole of season 2 (a season pack) must NOT
        // hold a file parsed as S02E01 — the pack legitimately contains it. But a
        // file from a DIFFERENT season is a real disagreement.
        let title = [Co::SeasonPack { season: 2 }];
        let refs: Vec<&Co> = title.iter().collect();
        assert!(
            coords_agree(&refs, &ep(2, 1)),
            "an episode of the packed season agrees with the season-pack intent"
        );
        assert!(
            coords_agree(&refs, &ep(2, 13)),
            "any episode of the packed season agrees"
        );
        assert!(
            !coords_agree(&refs, &ep(3, 1)),
            "an episode of a DIFFERENT season must not agree with the pack intent"
        );
    }

    #[test]
    fn coords_agree_non_episode_non_movie_requires_exact_membership() {
        // For coordinate kinds that are neither Movie nor Episode (e.g. a track),
        // agreement is exact membership in the title coordinates.
        let track = Co::Track { disc: 1, track: 4 };
        let title = [track.clone()];
        let refs: Vec<&Co> = title.iter().collect();
        assert!(coords_agree(&refs, &track));
        assert!(!coords_agree(&refs, &Co::Track { disc: 1, track: 5 }));
    }

    // --- verify_second_parse: holds on a true disagreement, passes otherwise --

    fn parsed_with_coords(coords: Vec<Co>) -> ParsedRelease {
        let mut p = ParsedRelease::new("x");
        p.coordinates = coords;
        p
    }

    #[test]
    fn verify_second_parse_passes_when_title_has_no_coordinates() {
        // No title coordinates -> nothing to contradict (movies, bare names).
        let title = parsed_with_coords(vec![]);
        let sources = [std::path::PathBuf::from("Some.Movie.2020.1080p.mkv")];
        assert!(verify_second_parse(&title, &sources).is_ok());
    }

    #[test]
    fn verify_second_parse_passes_when_file_name_carries_no_numbering() {
        // The title names S02E05 but the on-disk file is bare; with no file-side
        // coordinates there is nothing to contradict, so it passes.
        let title = parsed_with_coords(vec![ep(2, 5)]);
        let sources = [std::path::PathBuf::from("bare_filename.mkv")];
        assert!(verify_second_parse(&title, &sources).is_ok());
    }

    #[test]
    fn verify_second_parse_agrees_on_matching_episode() {
        let title = parsed_with_coords(vec![ep(2, 5)]);
        let sources = [std::path::PathBuf::from("The.Show.S02E05.1080p.WEB-DL.mkv")];
        assert!(
            verify_second_parse(&title, &sources).is_ok(),
            "the file name names the same episode the grab intended"
        );
    }

    #[test]
    fn verify_second_parse_holds_on_a_real_episode_disagreement() {
        // The grab intent was S02E05 but the on-disk file is S07E11 — a genuine
        // mismatch that must HOLD the import rather than force-fit a misnamed move.
        let title = parsed_with_coords(vec![ep(2, 5)]);
        let sources = [std::path::PathBuf::from(
            "Wrong.Show.S07E11.1080p.WEB-DL.mkv",
        )];
        let err = verify_second_parse(&title, &sources)
            .expect_err("a season/episode disagreement must hold the import");
        assert!(
            err.contains("disagrees"),
            "the hold reason names the conflict"
        );
    }

    #[test]
    fn verify_second_parse_season_pack_intent_accepts_per_episode_files() {
        // A season-pack grab whose files are individual episodes of that season
        // must import without holding (the pack contains them).
        let title = parsed_with_coords(vec![Co::SeasonPack { season: 2 }]);
        let sources = [
            std::path::PathBuf::from("The.Show.S02E01.1080p.WEB-DL.mkv"),
            std::path::PathBuf::from("The.Show.S02E02.1080p.WEB-DL.mkv"),
        ];
        assert!(verify_second_parse(&title, &sources).is_ok());

        // ...but a file from a different season inside the same batch holds.
        let mixed = [
            std::path::PathBuf::from("The.Show.S02E01.1080p.WEB-DL.mkv"),
            std::path::PathBuf::from("The.Show.S05E01.1080p.WEB-DL.mkv"),
        ];
        assert!(verify_second_parse(&title, &mixed).is_err());
    }

    // --- collect_sources: a file vs a directory ------------------------------

    #[test]
    fn collect_sources_returns_a_single_file_as_itself() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("movie.mkv");
        std::fs::write(&file, b"data").unwrap();
        let sources = collect_sources(&file).unwrap();
        assert_eq!(sources, vec![file]);
    }

    #[test]
    fn collect_sources_walks_a_directory_and_sorts_files() {
        let dir = tempfile::tempdir().unwrap();
        // Create out of lexical order to prove the result is sorted.
        for name in ["c.mkv", "a.mkv", "b.mkv"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        // A nested directory is NOT descended into (one level only).
        let sub = dir.path().join("subdir");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("deep.mkv"), b"x").unwrap();

        let sources = collect_sources(dir.path()).unwrap();
        let names: Vec<String> = sources
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["a.mkv", "b.mkv", "c.mkv"],
            "only top-level files, sorted, no recursion into subdir"
        );
    }
}
