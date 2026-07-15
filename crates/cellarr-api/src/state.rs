//! Shared application state handed to every handler.
//!
//! Dependencies are injected (no global mutable state, per the conventions):
//! the database handle, the command scheduler, the live-event bus, and the auth
//! configuration. `AppState` is cheap to clone (everything inside is an `Arc` or
//! a cloneable handle) and is the axum extractor state for all routers.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use cellarr_db::Database;

use cellarr_jobs::JobHandler;

use crate::auth::AuthConfig;
use crate::commands::{build_scheduler, build_scheduler_with, ApiScheduler};
use crate::events::EventBus;
use crate::import_list_sync::ImportListSyncRunner;
use crate::manual_import::ManualImport;
use crate::metadata::MetadataLookup;
use crate::queue::QueueDownloadClient;
use crate::release_search::{ReleaseGrab, ReleaseSearch};
use crate::tags::TagStore;

/// How long a cached artwork-presence snapshot is trusted before the next list
/// request rebuilds it. The artwork dir can live on a high-latency mount (CIFS), so
/// a per-request `read_dir` on the hot list path costs hundreds of ms; a short TTL
/// amortizes that to at most one `read_dir` per window while staying fresh enough
/// that newly-cached artwork appears within a few seconds.
const ARTWORK_CACHE_TTL: Duration = Duration::from_secs(30);

/// A time-bounded snapshot of which content ids have a cached-artwork directory.
///
/// The list endpoints consult this instead of doing a `read_dir` of the (possibly
/// network-mounted) MediaCover dir on every request. `built` is `None` until the
/// first build, so a fresh boot / empty cache is simply "stale" and triggers a
/// rebuild on first read.
#[derive(Default)]
pub struct CachedArtwork {
    /// The content-id subdirectory names present under the artwork dir at `built`.
    pub ids: HashSet<String>,
    /// When `ids` was last rebuilt; `None` before the first build.
    pub built: Option<Instant>,
}

impl CachedArtwork {
    /// Whether the snapshot is missing or older than the TTL and should be rebuilt.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        match self.built {
            Some(at) => at.elapsed() >= ARTWORK_CACHE_TTL,
            None => true,
        }
    }
}

/// The injected dependency bundle for the API.
#[derive(Clone)]
pub struct AppState {
    /// The persistence layer (reads via repos, writes via the writer-actor).
    pub db: Database,
    /// The live push-event bus, driven by real domain transitions.
    pub events: EventBus,
    /// The command scheduler (search/import/refresh go through here).
    pub scheduler: Arc<ApiScheduler>,
    /// API-key auth configuration.
    pub auth: Arc<AuthConfig>,
    /// The `/api/v3` tag store. cellarr's core has no tag domain yet, so the
    /// shim's `tag` CRUD (which Overseerr/Bazarr round-trip) is backed by this
    /// small in-process store rather than touching the persistence layer.
    pub tags: TagStore,
    /// The metadata-lookup seam the shim's `series/lookup`/`movie/lookup`
    /// resolve real identities through. `None` means no metadata wiring is
    /// configured at all (every lookup then reports unavailable); the wiring
    /// crate injects a live implementation over `cellarr-meta`.
    pub metadata: Option<Arc<dyn MetadataLookup>>,
    /// The interactive release-search seam `GET /api/v3/release` reads from.
    /// `None` means no pipeline wiring at all (the shim then reports every
    /// interactive search as unavailable); the daemon injects a live
    /// implementation over the real [`PipelineRunner`](cellarr_jobs::PipelineRunner).
    pub release_search: Option<Arc<dyn ReleaseSearch>>,
    /// The interactive grab seam `POST /api/v3/release` drives. `None` means no
    /// pipeline wiring at all (the shim then reports every grab as unavailable);
    /// the daemon injects a live implementation that builds the download client and
    /// drives the real [`PipelineRunner`](cellarr_jobs::PipelineRunner) Grab→Import
    /// path.
    pub release_grab: Option<Arc<dyn ReleaseGrab>>,
    /// The manual-import seam `GET/POST /api/v3/manualimport` reads from. `None`
    /// means no pipeline wiring at all (the shim then reports every scan/commit as
    /// unavailable); the daemon injects a live implementation over the real
    /// [`PipelineRunner`](cellarr_jobs::PipelineRunner) scan + crash-safe import path.
    pub manual_import: Option<Arc<dyn ManualImport>>,
    /// The import-list **sync** seam `POST /api/v3/importlist/{id}/sync` and the
    /// `ImportListSync` command drive. `None` means no sync wiring at all (the
    /// shim then reports a sync trigger as accepted-but-unwired); the daemon
    /// injects a live implementation over [`cellarr_jobs::ImportListSync`].
    pub import_list_sync: Option<Arc<dyn ImportListSyncRunner>>,
    /// The "remove a download from its client" seam the `DELETE /api/v3/queue/{id}`
    /// `removeFromClient` action drives. `None` means no download-client wiring (the
    /// queue row is still removed; the client removal is reported not-performed);
    /// the daemon injects a live implementation that builds the configured client.
    pub queue_client: Option<Arc<dyn QueueDownloadClient>>,
    /// The live download-client reachability probe backing the
    /// `download-client-unreachable` health check. `None` (offline/test) skips that
    /// check; the daemon injects an implementation that builds each configured
    /// client and pings it.
    pub download_client_probe: Option<Arc<dyn crate::health::DownloadClientProbe>>,
    /// The on-disk artwork cache directory (`<data_dir>/MediaCover`), where the
    /// identify/refresh path caches poster/fanart bytes keyed by content id. The
    /// `GET /api/v3/mediacover/{id}/{kind}` route serves files from here. `None`
    /// (the default) disables artwork serving (the route then always 404s) — the
    /// offline/test path; the daemon injects the real dir.
    pub artwork_dir: Option<PathBuf>,
    /// A short-TTL cache of which content ids have a cached-artwork directory under
    /// [`artwork_dir`](Self::artwork_dir). The list endpoints read this instead of
    /// doing a `read_dir` of the (possibly network-mounted) MediaCover dir on every
    /// request; it is rebuilt lazily when older than the TTL. Shared across clones
    /// (an `Arc`), so every request sees the same snapshot and only one rebuild runs
    /// per window. Starts empty/unbuilt, so a fresh boot rebuilds on first read.
    pub artwork_cache: Arc<RwLock<CachedArtwork>>,
    /// The recycle-bin directory a content delete moves media into instead of
    /// unlinking it (the media-management `recycleBinPath` setting). `None` (the
    /// default) makes a `deleteFiles` delete unlink outright; setting it makes the
    /// delete reversible — the bytes land under the bin, preserving their layout
    /// relative to the library root. The daemon injects the configured path.
    pub recycle_bin_path: Option<PathBuf>,
    /// The database backup/restore engine (`<data_dir>/backups`). `None` (the
    /// default) leaves the `/api/v3/system/backup` surface reporting backups
    /// unavailable — the offline/test path; the daemon injects the real engine
    /// bound to the live DB + its file path.
    pub backup: Option<Arc<crate::backup::BackupEngine>>,
    /// The on-disk log-file reader (`<data_dir>/logs`). `None` (the default)
    /// leaves the `/api/v3/log/file` surface reporting no log files; the daemon
    /// injects the reader bound to the rolling appender's directory.
    pub log_files: Option<Arc<crate::logfile::LogFiles>>,
}

impl AppState {
    /// Build the state from its parts, using the default event-only command
    /// handler. The scheduler is constructed here so it shares the one
    /// [`EventBus`] every other component publishes to.
    ///
    /// This is the API's own (offline/test) assembly: a submitted command runs a
    /// real job that publishes its domain event, but does no search/grab/import.
    /// The daemon injects a live pipeline handler via [`AppState::new_with_handler`].
    #[must_use]
    pub fn new(db: Database, auth: AuthConfig) -> Self {
        let events = EventBus::default();
        let scheduler = Arc::new(build_scheduler(events.clone()));
        Self::from_parts(db, auth, events, scheduler)
    }

    /// Build the state with an **injected** [`JobHandler`], so the daemon's live
    /// pipeline handler drives both the cron jobs (RssSync/MissingItemSearch) and
    /// a manual `/api/v3/command` search through the real pipeline.
    ///
    /// `make_handler` is given the shared [`EventBus`] so the handler publishes
    /// the same [`DomainEvent`](crate::events::DomainEvent)s the UI/WS observe,
    /// onto the one bus every other component uses.
    #[must_use]
    pub fn new_with_handler<F>(db: Database, auth: AuthConfig, make_handler: F) -> Self
    where
        F: FnOnce(EventBus) -> Arc<dyn JobHandler>,
    {
        let events = EventBus::default();
        let scheduler = Arc::new(build_scheduler_with(make_handler(events.clone())));
        Self::from_parts(db, auth, events, scheduler)
    }

    fn from_parts(
        db: Database,
        auth: AuthConfig,
        events: EventBus,
        scheduler: Arc<ApiScheduler>,
    ) -> Self {
        Self {
            db,
            events,
            scheduler,
            auth: Arc::new(auth),
            tags: TagStore::default(),
            metadata: None,
            release_search: None,
            release_grab: None,
            manual_import: None,
            import_list_sync: None,
            queue_client: None,
            download_client_probe: None,
            artwork_dir: None,
            artwork_cache: Arc::new(RwLock::new(CachedArtwork::default())),
            recycle_bin_path: None,
            backup: None,
            log_files: None,
        }
    }

    /// Attach a metadata-lookup source (the live `cellarr-meta` wiring), so the
    /// v3 lookup/list resources resolve real titles and external ids. Builder
    /// form so the base [`AppState::new`] stays zero-config (offline) and tests
    /// can opt a mock in.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Arc<dyn MetadataLookup>) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Attach the interactive release-search source (the live pipeline wiring), so
    /// `GET /api/v3/release` returns real ranked candidates. Builder form so the
    /// base [`AppState::new`] stays offline (the shim reports searches
    /// unavailable) and tests can opt a fake in.
    #[must_use]
    pub fn with_release_search(mut self, release_search: Arc<dyn ReleaseSearch>) -> Self {
        self.release_search = Some(release_search);
        self
    }

    /// Attach the interactive grab source (the live pipeline wiring), so
    /// `POST /api/v3/release` grabs the chosen release through the real download
    /// client. Builder form so the base [`AppState::new`] stays offline (the shim
    /// reports grabs unavailable) and tests can opt a fake in.
    #[must_use]
    pub fn with_release_grab(mut self, release_grab: Arc<dyn ReleaseGrab>) -> Self {
        self.release_grab = Some(release_grab);
        self
    }

    /// Attach the manual-import source (the live pipeline wiring), so
    /// `GET/POST /api/v3/manualimport` scans loose folders and commits chosen files
    /// through the real crash-safe import path. Builder form so the base
    /// [`AppState::new`] stays offline (the shim reports manual import unavailable)
    /// and tests can opt a fake in.
    #[must_use]
    pub fn with_manual_import(mut self, manual_import: Arc<dyn ManualImport>) -> Self {
        self.manual_import = Some(manual_import);
        self
    }

    /// Attach the import-list sync source (the live `cellarr-jobs` wiring), so
    /// `POST /api/v3/importlist/{id}/sync` and the `ImportListSync` command run the
    /// real safeguarded fetch+add. Builder form so the base [`AppState::new`] stays
    /// offline (the shim reports a sync trigger unavailable) and tests can opt a
    /// fake in.
    #[must_use]
    pub fn with_import_list_sync(mut self, runner: Arc<dyn ImportListSyncRunner>) -> Self {
        self.import_list_sync = Some(runner);
        self
    }

    /// Attach the queue download-client source (the live wiring), so a
    /// `DELETE /api/v3/queue/{id}?removeFromClient=true` actually removes the
    /// download from its client. Builder form so the base [`AppState::new`] stays
    /// offline (the removal is reported not-performed) and tests can opt a fake in.
    #[must_use]
    pub fn with_queue_client(mut self, client: Arc<dyn QueueDownloadClient>) -> Self {
        self.queue_client = Some(client);
        self
    }

    /// Set the live download-client reachability probe backing the
    /// `download-client-unreachable` health check.
    #[must_use]
    pub fn with_download_client_probe(
        mut self,
        probe: Arc<dyn crate::health::DownloadClientProbe>,
    ) -> Self {
        self.download_client_probe = Some(probe);
        self
    }

    /// Set the artwork cache directory (`<data_dir>/MediaCover`) the `MediaCover`
    /// route serves poster/fanart bytes from. Builder form so the base
    /// [`AppState::new`] stays artwork-less (the route 404s) and the daemon opts
    /// the real dir in.
    #[must_use]
    pub fn with_artwork_dir(mut self, dir: PathBuf) -> Self {
        self.artwork_dir = Some(dir);
        self
    }

    /// Set the recycle-bin directory a `deleteFiles` content delete moves media
    /// into (the media-management `recycleBinPath` setting). Builder form so the
    /// base [`AppState::new`] unlinks outright and the daemon opts a reversible
    /// recycle bin in.
    #[must_use]
    pub fn with_recycle_bin_path(mut self, dir: PathBuf) -> Self {
        self.recycle_bin_path = Some(dir);
        self
    }

    /// Attach the database backup/restore engine (`<data_dir>/backups`), enabling
    /// the `/api/v3/system/backup` surface. Builder form so the base
    /// [`AppState::new`] stays backup-less (the routes report unavailable) and the
    /// daemon opts the real engine in.
    #[must_use]
    pub fn with_backup(mut self, engine: crate::backup::BackupEngine) -> Self {
        self.backup = Some(Arc::new(engine));
        self
    }

    /// Attach the on-disk log-file reader (`<data_dir>/logs`), enabling the
    /// `/api/v3/log/file` surface. Builder form so the base [`AppState::new`]
    /// stays log-file-less and the daemon opts the rolling appender's dir in.
    #[must_use]
    pub fn with_log_files(mut self, reader: crate::logfile::LogFiles) -> Self {
        self.log_files = Some(Arc::new(reader));
        self
    }
}
