//! The command surface: turning an API request into a `cellarr-jobs` job.
//!
//! Native and shim "command" endpoints (trigger a search/import/refresh) submit
//! work through the scheduler rather than doing it inline (docs/09-api.md). This
//! module owns the concrete scheduler type the API drives and a [`JobHandler`]
//! that bridges a fired job back onto the [`EventBus`] as a domain transition.

use std::sync::Arc;

use cellarr_jobs::{
    ConcurrencyCaps, Job, JobHandler, JobKind, JobResult, JobStore, MemoryJobStore, RetryPolicy,
    Scheduler, SystemClock,
};

use crate::events::{DomainEvent, EventBus};

/// The handler the API's scheduler runs. It does not perform the heavy pipeline
/// work itself — in this surface its job is to mark the command observable by
/// emitting the matching domain event. The full pipeline handler from
/// `cellarr-jobs`/the daemon is wired in production; this keeps the API crate's
/// command path real (a job actually runs) without depending on the whole
/// pipeline assembly.
pub struct CommandHandler {
    events: EventBus,
}

impl CommandHandler {
    /// Build a handler that publishes onto `events`.
    #[must_use]
    pub fn new(events: EventBus) -> Self {
        Self { events }
    }
}

#[async_trait::async_trait]
impl JobHandler for CommandHandler {
    async fn handle(&self, kind: &JobKind) -> JobResult {
        // A fired command is a real domain transition: surface it to listeners.
        self.events.publish(DomainEvent::CommandQueued {
            job_id: String::new(),
            name: command_name(kind).to_string(),
        });
        JobResult::Success
    }
}

/// The concrete scheduler the API holds: system clock, in-memory store, the
/// event-bridging handler. The store is in-memory because the API process is the
/// long-lived daemon; a `cellarr-db`-backed store can be swapped behind the same
/// `JobStore` seam without touching handlers.
pub type ApiScheduler = Scheduler<SystemClock, MemoryJobStore, CommandHandler>;

/// Build the scheduler used by the command endpoints.
#[must_use]
pub fn build_scheduler(events: EventBus) -> ApiScheduler {
    Scheduler::new(
        Arc::new(SystemClock),
        Arc::new(MemoryJobStore::new()),
        Arc::new(CommandHandler::new(events)),
        ConcurrencyCaps::default(),
    )
}

/// The stable command name for a job kind, used in API responses and events.
#[must_use]
pub fn command_name(kind: &JobKind) -> &'static str {
    match kind {
        JobKind::RssSync => "RssSync",
        JobKind::MissingItemSearch => "MissingItemSearch",
        JobKind::MetadataRefresh => "RefreshMetadata",
        JobKind::DiskSpaceCheck => "DiskSpaceCheck",
        JobKind::ManualSearch { .. } => "ManualSearch",
    }
}

/// Map a command name (as sent by native or shim clients) to a [`JobKind`].
///
/// Accepts both cellarr's native names and the Radarr/Sonarr v3 command names
/// the ecosystem sends, so the `/api/v3` shim and the native API share one
/// mapping. Returns `None` for an unknown command.
#[must_use]
pub fn kind_for_command(name: &str, content_id: Option<String>) -> Option<JobKind> {
    // Case-insensitive on the leading token; ecosystem clients are inconsistent.
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "rsssync" => Some(JobKind::RssSync),
        // Both apps call a blanket "search for everything missing" command.
        "missingitemsearch" | "missingmoviessearch" | "missingepisodesearch" => {
            Some(JobKind::MissingItemSearch)
        }
        "refreshmetadata" | "refreshmovie" | "refreshseries" => Some(JobKind::MetadataRefresh),
        "diskspacecheck" => Some(JobKind::DiskSpaceCheck),
        "manualsearch" | "moviesearch" | "episodesearch" | "seriessearch" => {
            content_id.map(|content_id| JobKind::ManualSearch { content_id })
        }
        _ => None,
    }
}

/// Submit a command, returning the scheduler job id. A single `tick` is driven
/// so a submitted command actually fires its handler (and thus its event) rather
/// than waiting for an external tick loop; this keeps the command observable in
/// request scope.
///
/// # Errors
/// Returns the store error as a string on failure.
pub async fn submit(scheduler: &ApiScheduler, kind: JobKind) -> Result<String, String> {
    let id = scheduler
        .submit_now(kind, RetryPolicy::default())
        .await
        .map_err(|e| e.to_string())?;
    // Fire due jobs now so the command runs and emits its event in-band.
    scheduler.tick().await.map_err(|e| e.to_string())?;
    Ok(id)
}

/// Snapshot of the scheduler's jobs, for the system/queue views.
///
/// # Errors
/// Returns the store error as a string on failure.
pub async fn list_jobs(scheduler: &ApiScheduler) -> Result<Vec<Job>, String> {
    scheduler
        .store()
        .load_all()
        .await
        .map_err(|e| e.to_string())
}
