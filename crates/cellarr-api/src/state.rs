//! Shared application state handed to every handler.
//!
//! Dependencies are injected (no global mutable state, per the conventions):
//! the database handle, the command scheduler, the live-event bus, and the auth
//! configuration. `AppState` is cheap to clone (everything inside is an `Arc` or
//! a cloneable handle) and is the axum extractor state for all routers.

use std::sync::Arc;

use cellarr_db::Database;

use crate::auth::AuthConfig;
use crate::commands::{build_scheduler, ApiScheduler};
use crate::events::EventBus;
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
}

impl AppState {
    /// Build the state from its parts. The scheduler is constructed here so it
    /// shares the one [`EventBus`] every other component publishes to.
    #[must_use]
    pub fn new(db: Database, auth: AuthConfig) -> Self {
        let events = EventBus::default();
        let scheduler = Arc::new(build_scheduler(events.clone()));
        Self {
            db,
            events,
            scheduler,
            auth: Arc::new(auth),
            tags: TagStore::default(),
        }
    }
}
