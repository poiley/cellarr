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
//!   row is moved to [`GrabStatus::Failed`]/[`GrabStatus::Blocklisted`] so a
//!   re-search never re-grabs the same bad release.
//! - Import failures → [`Stage::HeldForReview`] (`import-failed → hold`).

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
use crate::notify::WebhookNotifier;

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
    /// The anime scene-mapping provider, used at Identify to remap an absolute
    /// episode number to its season/episode (TheXEM, behind the seam). `None`
    /// (the default) means no remap is attempted — a release that still carries an
    /// absolute coordinate is then surfaced for manual resolution rather than
    /// guessed, so the absence of a provider is safe.
    scene_provider: Option<Arc<dyn cellarr_media::DynSceneMappingProvider>>,
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
            scene_provider: None,
        }
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

        // The pipeline considers the candidates in indexer order; the first that
        // is identified and not rejected is the one grabbed. A reject is logged
        // and the next candidate tried (grab-failed → next release).
        let mut last_reject: Option<String> = None;
        for release in &releases {
            // --- Parse (real cellarr-parse on the *title*) ----------------
            let parsed = cellarr_parse::parse_title(&release.title);
            // (already advanced to Parse above for the first candidate; for
            // subsequent candidates the machine logically re-enters at Parse —
            // we keep `stage` at the furthest point reached so transitions stay
            // legal and the log reads as one run.)

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
                    return Ok(RunOutcome::HeldForReview { reason });
                }
            };

            // --- Identify (delegated to the media module) -----------------
            let matches = match self.identify(content, &parsed).await {
                Ok(m) => m,
                Err(e) => {
                    self.log(
                        run_id,
                        Stage::Identify,
                        Stage::HeldForReview,
                        TransitionKind::Hold,
                        None,
                        Some(format!("identify failed: {e}")),
                    )
                    .await?;
                    return Ok(RunOutcome::HeldForReview {
                        reason: format!("identify failed: {e}"),
                    });
                }
            };
            if stage == Stage::Parse {
                stage = self.advance(run_id, stage, None).await?; // -> Identify
            }
            let Some(matched) = self.best_match(content, matches) else {
                last_reject = Some("no confident content match".into());
                continue;
            };
            if stage == Stage::Identify {
                stage = self.advance(run_id, stage, None).await?; // -> Decide
            }

            // --- Blocklist consultation (before Decide) -------------------
            // A previously-failed release for this content must never be
            // re-grabbed; skip it and try the next candidate (the
            // download-failed -> blocklist + re-search transition). The decision
            // engine also hard-rejects a blocklisted release, so we pass the
            // membership through to keep the reject reason precise and logged.
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
                last_reject = Some(note);
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
                    last_reject = Some(note);
                    // Try the next candidate; the loop's exhaustion returns the
                    // last logged reject.
                    continue;
                }
                Verdict::Grab { score } | Verdict::Upgrade { to: score, .. } => {
                    let score = *score;
                    // Log the grab verdict on the Decide->Grab advance.
                    return self
                        .grab_track_import(
                            run_id,
                            &mut stage,
                            content,
                            &matched.content_ref,
                            release,
                            &parsed,
                            decision,
                            score,
                        )
                        .await;
                }
            }
        }

        // No candidate was grabbed. The run ends Rejected (the last logged
        // reason) — a normal, fully-logged outcome.
        let reason = last_reject.unwrap_or_else(|| "no acceptable release".into());
        Ok(RunOutcome::Rejected { reason })
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
    ) -> Result<RunOutcome> {
        // --- Grab: persist the grab, hand to the download client ----------
        // Derive the durable release type from the title parse ONCE, here, and
        // persist it on the grab. Everything downstream (media_file, history,
        // reconcile) reads this back instead of re-parsing the title.
        let release_type = cellarr_core::ReleaseType::from_parsed(parsed);
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
                return self
                    .fail_grab(
                        run_id,
                        content.id,
                        grab_id,
                        GrabStatus::Failed,
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
        // + the grabbed release object Bazarr-push/Notifiarr read.
        self.fire_grab_webhook(matched_ref, release).await;

        *stage = self.advance(run_id, *stage, None).await?; // -> Track

        // --- Track: poll to completion, read content_path -----------------
        let content_path = match self.track(&download_id).await {
            Ok(Some(path)) => {
                // Shared remote-path remapping: the client reports the path from
                // its own vantage point; rewrite it to where cellarr can see it
                // before Import. Applied here, once, for every download client.
                cellarr_core::apply_remote_path_mappings(
                    &self.config.remote_path_mappings,
                    &self.config.client_host,
                    &path,
                )
            }
            Ok(None) => {
                return self
                    .fail_grab(
                        run_id,
                        content.id,
                        grab_id,
                        GrabStatus::Blocklisted,
                        "download completed without a content path".into(),
                        release,
                    )
                    .await;
            }
            Err(detail) => {
                return self
                    .fail_grab(
                        run_id,
                        content.id,
                        grab_id,
                        GrabStatus::Blocklisted,
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
                *stage = self.advance(run_id, *stage, None).await?; // -> Rename
                self.fire_files_webhook(WebhookEventType::Rename, matched_ref, &destinations)
                    .await;
                *stage = self.advance(run_id, *stage, None).await?; // -> Notify
                self.append_history(run_id, content.id, HistoryEvent::Imported { grab_id })
                    .await?;
                self.advance(run_id, *stage, Some("imported".into()))
                    .await?; // -> Done
                Ok(RunOutcome::Imported {
                    grab_id,
                    destinations,
                })
            }
            Err(detail) => {
                // import-failed -> hold for review (never silently drop, never
                // force-fit a destructive write).
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
                Ok(RunOutcome::HeldForReview { reason: detail })
            }
        }
    }

    /// Poll the download client until completion or terminal failure.
    ///
    /// Returns `Ok(Some(path))` on completion with a content path,
    /// `Ok(None)` on completion with no path (a caller-handled anomaly), and
    /// `Err(detail)` on a failed download or exhausted polls. Uses a bounded
    /// poll count rather than a tight loop; the logical clock advances between
    /// polls so tests never sleep.
    async fn track(&self, download_id: &str) -> std::result::Result<Option<String>, String> {
        for _ in 0..self.config.max_track_polls {
            let status = self
                .client
                .status(download_id)
                .await
                .map_err(|e| format!("status poll failed: {e}"))?;
            match status.state {
                DownloadState::Completed => return Ok(status.content_path),
                DownloadState::Failed => return Err("download failed".into()),
                DownloadState::Queued | DownloadState::Downloading => {
                    // Event-driven progress is preferred (docs/03-pipeline.md);
                    // absent a webhook, poll with the (logical) clock advancing.
                    let _ = self.clock.now_secs();
                    tokio::task::yield_now().await;
                }
            }
        }
        Err("tracking timed out".into())
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
        Ok(result
            .moves
            .into_iter()
            .map(|m| m.destination_path.to_string_lossy().into_owned())
            .collect())
    }

    // --- Failure / transition / log helpers ------------------------------

    #[allow(clippy::too_many_arguments)]
    async fn fail_grab(
        &self,
        run_id: PipelineRunId,
        content_id: cellarr_core::ContentId,
        grab_id: GrabId,
        terminal: GrabStatus,
        detail: String,
        release: &Release,
    ) -> Result<RunOutcome> {
        self.set_grab_status(grab_id, terminal).await?;
        // A download/grab failure that reaches a Blocklisted terminal records the
        // release in the blocklist so a re-search never re-grabs it (the
        // download-failed -> blocklist + re-search transition). A plain Failed is
        // re-searchable and is not blocklisted.
        if terminal == GrabStatus::Blocklisted {
            self.blocklist_release(content_id, release, &detail).await?;
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
        Ok(RunOutcome::Failed { detail })
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
