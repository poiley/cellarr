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

/// The scheduler the API holds: system clock, in-memory store, and an
/// **injected** [`JobHandler`] held as a trait object.
///
/// The handler is type-erased (`Arc<dyn JobHandler>`) so the *daemon* can inject
/// the live pipeline handler (search→grab→import) while keeping the same
/// scheduler type the API constructs by default. The store is in-memory because
/// the API process is the long-lived daemon; a `cellarr-db`-backed store can be
/// swapped behind the same `JobStore` seam without touching handlers.
pub type ApiScheduler = Scheduler<SystemClock, MemoryJobStore, Arc<dyn JobHandler>>;

/// Build the scheduler with the default event-only [`CommandHandler`].
///
/// This keeps the API crate self-contained (its own tests get a real scheduler
/// whose jobs actually fire and publish events) without depending on the whole
/// pipeline assembly. The daemon overrides the handler via
/// [`build_scheduler_with`].
#[must_use]
pub fn build_scheduler(events: EventBus) -> ApiScheduler {
    build_scheduler_with(Arc::new(CommandHandler::new(events)))
}

/// Build the scheduler over an injected [`JobHandler`].
///
/// The daemon passes its `LivePipelineHandler` here so both the cron jobs
/// (RssSync/MissingItemSearch) and a manual `/api/v3/command` search drive the
/// real pipeline; the API's own tests pass the event-only [`CommandHandler`].
#[must_use]
pub fn build_scheduler_with(handler: Arc<dyn JobHandler>) -> ApiScheduler {
    Scheduler::new(
        Arc::new(SystemClock),
        Arc::new(MemoryJobStore::new()),
        Arc::new(handler),
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
        // `refreshcontent` is cellarr's content-scoped refresh name (the UI's
        // "Refresh Content" action); accept it as an alias for the metadata
        // refresh alongside the *arr-native names so the command is valid.
        "refreshmetadata" | "refreshmovie" | "refreshseries" | "refreshcontent" => {
            Some(JobKind::MetadataRefresh)
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refreshcontent_maps_to_metadata_refresh() {
        // cellarr's content-scoped refresh name resolves to MetadataRefresh, so a
        // "Refresh Content" command is a valid command rather than an unknown one.
        assert!(matches!(
            kind_for_command("refreshcontent", None),
            Some(JobKind::MetadataRefresh)
        ));
        // Case-insensitive on the leading token, like the other commands.
        assert!(matches!(
            kind_for_command("RefreshContent", None),
            Some(JobKind::MetadataRefresh)
        ));
    }

    #[test]
    fn existing_refresh_names_still_map() {
        for name in ["refreshmetadata", "refreshmovie", "refreshseries"] {
            assert!(
                matches!(kind_for_command(name, None), Some(JobKind::MetadataRefresh)),
                "{name} should still map to MetadataRefresh"
            );
        }
    }

    #[test]
    fn unknown_command_is_none() {
        assert!(kind_for_command("definitelynotacommand", None).is_none());
    }
}
