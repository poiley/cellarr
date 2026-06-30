//! The boot sequence and graceful shutdown — the daemon's spine.
//!
//! Order (`docs/01-architecture.md`, `docs/specs/cellarr-cli.md`):
//! open the database (which runs migrations) → build the media registry →
//! start the jobs scheduler → start the API server. On a shutdown signal we
//! stop the scheduler, stop serving, then **drain the writer-actor and close the
//! pool** so the database is left consistent (no torn writes).
//!
//! Everything is injected; there is no global state. The function returns when
//! the server has stopped and the database has been closed cleanly.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use cellarr_api::{AppState, AuthConfig};
use cellarr_db::Database;
use cellarr_jobs::JobKind;
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::config::Config;
use crate::registry::build_media_registry;

/// How often the scheduler is ticked in the running daemon. Recurring jobs fire
/// on their own schedule; this is just the cadence at which due jobs are picked
/// up. Kept modest so an idle daemon is quiet.
const SCHEDULER_TICK_SECS: u64 = 5;

/// How often the daemon takes an automatic backup (daily).
const BACKUP_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// How many automatic backups to retain (older ones are pruned each run).
const BACKUP_RETENTION: usize = 7;

/// A booted daemon: the bound listener, the assembled state, and the resolved
/// address. Split from [`run`] so a test can boot, learn the real port, talk to
/// the server, and trigger shutdown deterministically rather than racing a signal.
pub struct Daemon {
    listener: TcpListener,
    state: AppState,
    addr: SocketAddr,
}

impl Daemon {
    /// Run the full boot sequence and bind the API listener, but do not serve yet.
    ///
    /// Binding here (not inside [`serve_until`]) lets the caller read back the OS-
    /// assigned port when configured with port `0`.
    ///
    /// # Errors
    /// Propagates database, registry, or bind failures with context.
    pub async fn boot(config: &Config) -> Result<Self> {
        // 1. Data dir + database (migrations run inside `Database::open`).
        std::fs::create_dir_all(&config.data_dir)
            .with_context(|| format!("creating data dir {}", config.data_dir.display()))?;
        let db_path = config.database_path();
        let db = Database::open(
            db_path
                .to_str()
                .context("database path is not valid UTF-8")?,
        )
        .await
        .with_context(|| format!("opening database at {}", db_path.display()))?;
        info!(path = %db_path.display(), "database open; migrations applied");

        // 1b. Managed config (config-as-code). If a path is configured, reconcile
        //     the DB to match the declarative file now — after migrations, before
        //     anything serves — so the daemon never serves a half-applied or stale
        //     config. A managed-config error FAILS boot loudly (we propagate it);
        //     with no path configured this is a no-op and behaviour is unchanged
        //     (zero-config startup still works). Pruning only ever removes entities
        //     config previously managed, never UI-created ones.
        if let Some(path) = &config.managed_config_path {
            info!(path = %path.display(), "reconciling managed config");
            let report = crate::managed::reconcile_on_boot(&db, path)
                .await
                .with_context(|| format!("reconciling managed config from {}", path.display()))?;
            let total = report.totals();
            info!(
                created = total.created,
                updated = total.updated,
                pruned = total.pruned,
                unchanged = total.unchanged,
                "managed config reconciled"
            );
        }

        // 2. Registries: the media-type modules the daemon serves. Held by the
        //    pipeline; built here because the CLI is the only wiring crate. The
        //    indexer/download/metadata adapters are constructed per-config from
        //    secrets in the DB on demand, so the registry the daemon owns at boot
        //    is the media registry (`docs/01-architecture.md`).
        let media = build_media_registry(&db);
        info!(
            media_types = media.media_types().count(),
            "media registry built"
        );

        // 2b. Metadata sources (bring-your-own-key, from `.env`/`CELLARR_*`).
        //     Construct the live TheTVDB (TV) + TMDb (movie) sources behind the
        //     API's metadata seam, so the v3 lookup/list resources resolve real
        //     titles + external ids. With no key a source reports unavailable and
        //     the daemon degrades gracefully (offline is non-negotiable). Keys are
        //     never logged.
        let metadata = std::sync::Arc::new(crate::metadata::LiveMetadata::from_config(config));
        info!(
            thetvdb_configured = config.tvdb.api_key.is_some(),
            thetvdb_pin = config.tvdb.pin.is_some(),
            tmdb_configured = config.tmdb.api_key.is_some(),
            "metadata sources initialized"
        );

        //     The anime scene-mapping provider (TheTVDB/TheXEM), constructed from
        //     the same meta-service config and shared (as a `dyn` trait object)
        //     across the automatic pipeline handler and the interactive
        //     search/grab seams. THIS is the dead-in-prod fix: attaching it makes
        //     the Identify-stage absolute→season/episode remap actually run in the
        //     daemon for anime-typed series. With no TheTVDB key it degrades to
        //     "no mapping", so an absolute release is surfaced for manual
        //     resolution rather than guessed (offline non-negotiable).
        let scene_provider: std::sync::Arc<dyn cellarr_media::DynSceneMappingProvider> =
            std::sync::Arc::new(crate::metadata::TvdbSceneMappings::from_config(config));

        // 3. State (which builds the command scheduler over a shared event bus).
        //    The live metadata seam is attached so `series/lookup`/`movie/lookup`
        //    resolve real identities.
        //
        //    Crucially, the daemon injects the **live pipeline handler** as the
        //    scheduler's `JobHandler`, so a fired job (a cron RssSync/MissingItem
        //    search, or a manual `/api/v3/command` search) drives the *real*
        //    pipeline — search → grab → import — rather than only emitting an
        //    event. The handler owns the shared media registry and reads the
        //    configured indexers + download client from the DB per run, and
        //    publishes the matching domain events onto the same bus the API's
        //    state.new() handler used (so the UI/WS still observe progress). The
        //    API crate keeps its event-only default handler for its own tests.
        let auth = match &config.api.api_key {
            Some(key) => AuthConfig::with_key(key.clone()),
            None => AuthConfig::disabled(),
        };
        let registry = std::sync::Arc::new(media);
        let handler_db = db.clone();
        let handler_registry = std::sync::Arc::clone(&registry);
        // The interactive release-search seam (GET /api/v3/release) shares the
        // same DB + media registry, driving the runner's read-only Discover→Decide
        // preview (no grab).
        let search_db = db.clone();
        let search_registry = std::sync::Arc::clone(&registry);
        let release_search = std::sync::Arc::new(
            crate::pipeline::LiveReleaseSearch::new(search_db, search_registry)
                .with_scene_provider(std::sync::Arc::clone(&scene_provider)),
        );
        // The interactive grab seam (POST /api/v3/release) shares the same DB +
        // media registry, but — unlike search — builds the download client and
        // drives the real Grab→Track→Import path for the chosen release. It needs
        // the shared event bus to surface progress, so it is attached after the
        // state (which owns the bus) is built.
        let grab_db = db.clone();
        let grab_registry = std::sync::Arc::clone(&registry);
        let handler_scene_provider = std::sync::Arc::clone(&scene_provider);

        // The artwork cache dir (`<data_dir>/MediaCover`): the v3 mediacover route
        // serves from it and the metadata resolver caches poster/fanart into it.
        // Created up front (before the handler) so neither the route's reads nor the
        // resolver's writes race a missing dir.
        let artwork_dir = config.data_dir.join("MediaCover");
        std::fs::create_dir_all(&artwork_dir)
            .with_context(|| format!("creating artwork dir {}", artwork_dir.display()))?;
        // The concrete metadata resolver the identify/refresh path resolves rich
        // metadata + artwork through (movies via TMDb, TV via TheTVDB). Attached to
        // the pipeline handler so `RefreshMetadata` (and a content-scoped refresh)
        // do real work; degrades to a no-op when a source has no key.
        let resolver: std::sync::Arc<dyn cellarr_media::DynMetadataResolver> =
            std::sync::Arc::new(crate::resolver::LiveMetadataResolver::from_config(
                config,
                db.clone(),
                artwork_dir.clone(),
            ));
        let handler_resolver = std::sync::Arc::clone(&resolver);

        let state = AppState::new_with_handler(db, auth, move |events| {
            let env = crate::pipeline::LivePipelineEnv::new(handler_db.clone());
            // Attach the scene-mapping provider so a fired job's Identify stage
            // runs the anime absolute→episode remap (the dead-in-prod fix).
            std::sync::Arc::new(
                crate::pipeline::LivePipelineHandler::new(
                    handler_db,
                    handler_registry,
                    events,
                    env,
                )
                .with_scene_provider(handler_scene_provider)
                .with_resolver(handler_resolver),
            )
        })
        .with_metadata(metadata)
        .with_release_search(release_search);
        let release_grab = std::sync::Arc::new(
            crate::pipeline::LiveReleaseGrab::new(grab_db, grab_registry, state.events.clone())
                .with_scene_provider(std::sync::Arc::clone(&scene_provider)),
        );
        let state = state.with_release_grab(release_grab);

        // The manual-import seam (GET/POST /api/v3/manualimport) shares the same DB +
        // media registry, scanning a loose folder (read-only) and committing chosen
        // files through the runner's crash-safe import path — it never grabs, so (like
        // search) it builds no download client.
        let manual_import = std::sync::Arc::new(crate::pipeline::LiveManualImport::new(
            state.db.clone(),
            std::sync::Arc::clone(&registry),
        ));
        let state = state.with_manual_import(manual_import);

        // The import-list sync seam (POST /api/v3/importlist/{id}/sync + the
        // ImportListSync command) runs the safeguarded fetch+add over the live
        // TMDb/IMDb/collection source factory; Trakt/Plex are credential-gated.
        let import_list_sync =
            std::sync::Arc::new(crate::pipeline::LiveImportListSync::new(state.db.clone()));
        let state = state.with_import_list_sync(import_list_sync);

        // The queue download-client seam (DELETE /api/v3/queue/{id}?removeFromClient=)
        // resolves the configured client per call to remove a download when a queue
        // item is removed.
        let queue_client =
            std::sync::Arc::new(crate::pipeline::LiveQueueClient::new(state.db.clone()));
        let state = state.with_queue_client(queue_client);

        // Wire the (already-created) artwork cache dir into the API state so the v3
        // `mediacover/{id}/{kind}` route serves the bytes the resolver caches.
        let state = state.with_artwork_dir(artwork_dir.clone());

        // The recycle bin a `deleteFiles` content delete moves removed media into
        // (making the delete reversible). Unset → deletes unlink outright.
        let state = match config.media_management.recycle_bin_path.clone() {
            Some(bin) => state.with_recycle_bin_path(bin),
            None => state,
        };

        // The database backup/restore engine, bound to `<data_dir>/backups` and the
        // live DB + its file path (the path is needed for the atomic restore swap).
        // This powers the `/api/v3/system/backup` surface and the scheduled backup
        // job below.
        // Bundle the MediaCover artwork into backups too, so a restore carries the
        // cached posters/fanart (the DB already carries the text metadata).
        let backup_engine =
            cellarr_api::BackupEngine::new(config.backup_dir(), state.db.clone(), db_path.clone())
                .with_artwork_dir(artwork_dir.clone());
        let state = state.with_backup(backup_engine);

        // The on-disk log-file reader, bound to the rolling appender's directory
        // (`<data_dir>/logs`), powering the `/api/v3/log/file` surface.
        let log_files = cellarr_api::LogFiles::new(config.log_dir());
        let state = state.with_log_files(log_files);

        // Register the recurring maintenance jobs the daemon runs unattended.
        register_recurring(&state).await?;

        // 4. Bind the API listener.
        let bind_addr = SocketAddr::new(config.api.bind, config.api.port);
        let listener = TcpListener::bind(bind_addr)
            .await
            .with_context(|| format!("binding API listener on {bind_addr}"))?;
        let addr = listener
            .local_addr()
            .context("reading bound listener address")?;
        info!(%addr, "API listener bound");

        Ok(Self {
            listener,
            state,
            addr,
        })
    }

    /// The address the API is bound to (with the real port resolved when `0` was
    /// configured).
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The assembled application state (for tests that want to reach the DB).
    #[must_use]
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Serve until `shutdown` resolves, then shut down gracefully: stop the
    /// scheduler tick loop, stop accepting, drain the writer-actor, and close the
    /// connection pool so the database is left consistent.
    ///
    /// # Errors
    /// Propagates a fatal serve error (a clean shutdown is `Ok`).
    pub async fn serve_until<F>(self, shutdown: F) -> Result<()>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let Daemon {
            listener, state, ..
        } = self;

        // Start the scheduler tick loop on its own task; a shutdown handle lets us
        // stop driving jobs before we drain the DB.
        let (tick_stop_tx, mut tick_stop_rx) = tokio::sync::oneshot::channel::<()>();
        let scheduler = state.scheduler.clone();
        let tick_task = tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(SCHEDULER_TICK_SECS));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(e) = scheduler.tick().await {
                            warn!(error = %e, "scheduler tick failed");
                        }
                    }
                    _ = &mut tick_stop_rx => break,
                }
            }
        });

        // The scheduled-backup task: take an automatic backup on a daily cadence
        // and prune to the retention bound. Runs on its own task with the same
        // shutdown handle shape as the scheduler tick. Skipped if no backup engine
        // is attached (it always is in the daemon). The first backup is taken one
        // interval in, not at boot, so a flapping restart loop does not spam
        // backups; the operator can still trigger one immediately via the API.
        let (backup_stop_tx, mut backup_stop_rx) = tokio::sync::oneshot::channel::<()>();
        let backup_engine = state.backup.clone();
        let backup_task = tokio::spawn(async move {
            let Some(engine) = backup_engine else {
                return;
            };
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(BACKUP_INTERVAL_SECS));
            // Consume the immediate first tick so the first backup lands after one
            // full interval rather than at startup.
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        match engine.create("scheduled", serde_json::Value::Null).await {
                            Ok(info) => {
                                info!(backup = %info.id, "scheduled backup created");
                                match engine.prune(BACKUP_RETENTION) {
                                    Ok(pruned) if !pruned.is_empty() => {
                                        info!(count = pruned.len(), "pruned old backups");
                                    }
                                    Ok(_) => {}
                                    Err(e) => warn!(error = %e, "backup retention prune failed"),
                                }
                            }
                            Err(e) => warn!(error = %e, "scheduled backup failed"),
                        }
                    }
                    _ = &mut backup_stop_rx => break,
                }
            }
        });

        // The DB handle to drain after the server stops. Cloning is cheap; the
        // pool is shared, so closing it here closes it everywhere.
        let db = state.db.clone();

        // Serve with axum's graceful-shutdown hook driven by the caller's signal.
        let serve_result = cellarr_api::serve_with_shutdown(listener, state, shutdown).await;

        // --- Graceful shutdown ------------------------------------------------
        // Stop ticking new jobs; in-flight ones (run sequentially within a tick)
        // are allowed to finish since a tick is awaited to completion.
        let _ = tick_stop_tx.send(());
        if let Err(e) = tick_task.await {
            warn!(error = %e, "scheduler task join failed during shutdown");
        }
        // Stop the scheduled-backup task too (an in-flight backup is allowed to
        // finish — `select!` only breaks at the next loop boundary).
        let _ = backup_stop_tx.send(());
        if let Err(e) = backup_task.await {
            warn!(error = %e, "backup task join failed during shutdown");
        }

        // Drain the writer-actor and close the pool, leaving a consistent DB.
        // `shutdown` stops the actor first so its dedicated connection is released
        // before the pool close waits on it (closing the pool directly deadlocks).
        db.shutdown().await;
        info!("database drained and closed");

        serve_result.context("API server error")
    }
}

/// Register the recurring maintenance jobs every daemon runs (RSS sync, missing-
/// item search, disk checks). Registration is deduplicated by the scheduler, so
/// this is idempotent across restarts.
async fn register_recurring(state: &AppState) -> Result<()> {
    use cellarr_jobs::RetryPolicy;

    let scheduler = &state.scheduler;
    scheduler
        .add_cron(JobKind::RssSync, "*/15 * * * *", RetryPolicy::default())
        .await
        .context("registering RssSync")?;
    scheduler
        .add_cron(JobKind::MissingItemSearch, "@daily", RetryPolicy::default())
        .await
        .context("registering MissingItemSearch")?;
    scheduler
        .add_cron(JobKind::DiskSpaceCheck, "@hourly", RetryPolicy::default())
        .await
        .context("registering DiskSpaceCheck")?;
    // Refresh rich metadata + artwork for the library daily. Without this the
    // resolver only runs on a manual/per-add refresh, so existing nodes never gain
    // overview/runtime/genres/ratings/posters on their own.
    scheduler
        .add_cron(JobKind::MetadataRefresh, "@daily", RetryPolicy::default())
        .await
        .context("registering MetadataRefresh")?;
    Ok(())
}

/// Run the daemon to completion, shutting down on Ctrl-C / SIGTERM.
///
/// # Errors
/// Propagates any boot or serve failure.
pub async fn run(config: Config) -> Result<()> {
    let daemon = Daemon::boot(&config).await?;
    info!(addr = %daemon.addr(), "cellarr daemon ready");
    daemon.serve_until(shutdown_signal()).await
}

/// Resolve when the process receives a shutdown signal (Ctrl-C, or SIGTERM on
/// Unix). Either trigger leads to the same graceful path.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => warn!(error = %e, "failed to install SIGTERM handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    info!("shutdown signal received");
}
