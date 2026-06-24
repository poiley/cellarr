//! Shared application state handed to every handler.
//!
//! Dependencies are injected (no global mutable state, per the conventions):
//! the database handle, the command scheduler, the live-event bus, and the auth
//! configuration. `AppState` is cheap to clone (everything inside is an `Arc` or
//! a cloneable handle) and is the axum extractor state for all routers.

use std::sync::Arc;

use cellarr_db::Database;

use cellarr_jobs::JobHandler;

use crate::auth::AuthConfig;
use crate::commands::{build_scheduler, build_scheduler_with, ApiScheduler};
use crate::events::EventBus;
use crate::metadata::MetadataLookup;
use crate::release_search::ReleaseSearch;
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
}
