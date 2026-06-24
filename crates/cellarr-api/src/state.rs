//! Shared application state handed to every handler.
//!
//! Dependencies are injected (no global mutable state, per the conventions):
//! the database handle, the command scheduler, the live-event bus, and the auth
//! configuration. `AppState` is cheap to clone (everything inside is an `Arc` or
//! a cloneable handle) and is the axum extractor state for all routers.

use std::path::PathBuf;
use std::sync::Arc;

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
    /// The on-disk artwork cache directory (`<data_dir>/MediaCover`), where the
    /// identify/refresh path caches poster/fanart bytes keyed by content id. The
    /// `GET /api/v3/mediacover/{id}/{kind}` route serves files from here. `None`
    /// (the default) disables artwork serving (the route then always 404s) — the
    /// offline/test path; the daemon injects the real dir.
    pub artwork_dir: Option<PathBuf>,
    /// The recycle-bin directory a content delete moves media into instead of
    /// unlinking it (the media-management `recycleBinPath` setting). `None` (the
    /// default) makes a `deleteFiles` delete unlink outright; setting it makes the
    /// delete reversible — the bytes land under the bin, preserving their layout
    /// relative to the library root. The daemon injects the configured path.
    pub recycle_bin_path: Option<PathBuf>,
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
            artwork_dir: None,
            recycle_bin_path: None,
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
}
