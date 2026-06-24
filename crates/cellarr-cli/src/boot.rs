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
        let release_search = std::sync::Arc::new(crate::pipeline::LiveReleaseSearch::new(
            search_db,
            search_registry,
        ));
        // The interactive grab seam (POST /api/v3/release) shares the same DB +
        // media registry, but — unlike search — builds the download client and
        // drives the real Grab→Track→Import path for the chosen release. It needs
        // the shared event bus to surface progress, so it is attached after the
        // state (which owns the bus) is built.
        let grab_db = db.clone();
        let grab_registry = std::sync::Arc::clone(&registry);
        let state = AppState::new_with_handler(db, auth, move |events| {
            let env = crate::pipeline::LivePipelineEnv::new(handler_db.clone());
            std::sync::Arc::new(crate::pipeline::LivePipelineHandler::new(
                handler_db,
                handler_registry,
                events,
                env,
            ))
        })
        .with_metadata(metadata)
        .with_release_search(release_search);
        let release_grab = std::sync::Arc::new(crate::pipeline::LiveReleaseGrab::new(
            grab_db,
            grab_registry,
            state.events.clone(),
        ));
        let state = state.with_release_grab(release_grab);

        // The artwork cache dir (`<data_dir>/MediaCover`) the identify/refresh path
        // caches poster/fanart into and the v3 `mediacover/{id}/{kind}` route serves
        // from. Created up front so the route's reads never race a missing dir.
        let artwork_dir = config.data_dir.join("MediaCover");
        std::fs::create_dir_all(&artwork_dir)
            .with_context(|| format!("creating artwork dir {}", artwork_dir.display()))?;
        let state = state.with_artwork_dir(artwork_dir);

        // The recycle bin a `deleteFiles` content delete moves removed media into
        // (making the delete reversible). Unset → deletes unlink outright.
        let state = match config.media_management.recycle_bin_path.clone() {
            Some(bin) => state.with_recycle_bin_path(bin),
            None => state,
        };

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
