//! The persisted job model and its store seam.
//!
//! A [`Job`] is a unit of scheduled work: a recurring schedule (RSS sync,
//! missing-item search, disk-space check) or an on-demand request (manual
//! search/import). Jobs are *persisted* (the [`JobStore`] seam) so they survive a
//! restart — on startup the scheduler reloads them and resumes their schedules.
//!
//! The model is deliberately serializable and time-relative (everything in whole
//! seconds on the injected [`crate::clock::Clock`]) so the persistence and
//! restart tests round-trip a store and assert the schedule continues, with no
//! real sleeps.

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// What a job *does* when it fires. Kept coarse and serializable; the runner is
/// invoked with the concrete content/seams the dispatcher resolves from this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobKind {
    /// Periodic RSS/latest sync against the configured indexers.
    RssSync,
    /// Search for monitored items that currently lack an acceptable file.
    MissingItemSearch,
    /// Refresh metadata for known content.
    MetadataRefresh,
    /// A disk-space / health check.
    DiskSpaceCheck,
    /// Reconcile on-disk files against the `media_file` table: adopt untracked
    /// files the scanner confidently identifies, and surface the rest for manual
    /// import. The in-place counterpart to import-time adopt.
    RescanLibrary,
    /// Reconcile in-flight grabs against reality: finalize or clean the downloads a
    /// pipeline run left non-terminal (a run that ended before its download
    /// resolved, or a duplicate grab for content another grab already satisfied).
    /// Without it such grabs linger forever as "downloading"/"importing" and a dead
    /// download is never removed from the client.
    ReconcileDownloads,
    /// An on-demand manual search for a specific content node.
    ManualSearch {
        /// The content node (UUID string) to search for.
        content_id: String,
    },
    /// Periodic sweep for missing subtitles across every content node that has a
    /// file: for each wanted language a file lacks, search a provider and fetch it.
    SubtitleScan,
    /// An on-demand subtitle search for one content node (the UI "Search
    /// subtitles" button).
    SubtitleSearch {
        /// The content node (UUID string) to fetch subtitles for.
        content_id: String,
    },
}

impl JobKind {
    /// The resource bucket this job contends on, for concurrency caps and rate
    /// limits. Jobs that hit the same third party share a bucket so the
    /// scheduler never stampedes it.
    #[must_use]
    pub fn resource(&self) -> &'static str {
        match self {
            JobKind::RssSync | JobKind::MissingItemSearch | JobKind::ManualSearch { .. } => {
                "indexer"
            }
            JobKind::MetadataRefresh => "metadata",
            JobKind::DiskSpaceCheck => "disk",
            // A rescan is filesystem-bound (scan + in-place adopt), sharing no third
            // party — its own bucket keeps it off the indexer/metadata budgets.
            JobKind::RescanLibrary => "filesystem",
            // Reconcile polls the download client, so share its bucket.
            JobKind::ReconcileDownloads => "download",
            // Subtitle jobs hit the subtitle provider — their own bucket keeps
            // them off the indexer/metadata budgets.
            JobKind::SubtitleScan | JobKind::SubtitleSearch { .. } => "subtitle",
        }
    }

    /// A stable identity used for **deduplication**: two submissions with the
    /// same dedup key are the same logical job and must not run concurrently.
    #[must_use]
    pub fn dedup_key(&self) -> String {
        match self {
            JobKind::RssSync => "rss_sync".into(),
            JobKind::MissingItemSearch => "missing_item_search".into(),
            JobKind::MetadataRefresh => "metadata_refresh".into(),
            JobKind::DiskSpaceCheck => "disk_space_check".into(),
            JobKind::RescanLibrary => "rescan_library".into(),
            JobKind::ReconcileDownloads => "reconcile_downloads".into(),
            JobKind::ManualSearch { content_id } => format!("manual_search:{content_id}"),
            JobKind::SubtitleScan => "subtitle_scan".into(),
            JobKind::SubtitleSearch { content_id } => format!("subtitle_search:{content_id}"),
        }
    }
}

/// How often a job fires. On-demand jobs use [`Schedule::Once`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "schedule", rename_all = "snake_case")]
pub enum Schedule {
    /// Fire once, as soon as it is due (`at` seconds on the clock).
    Once {
        /// The clock time the job is due.
        at: u64,
    },
    /// Fire every `interval_secs`, starting at `next` seconds on the clock.
    ///
    /// A fixed-interval schedule is the deterministic, logical-clock-friendly
    /// reduction of a cron expression: the dispatcher computes the next fire
    /// purely from these two numbers, so tests advance the clock and assert
    /// exactly which ticks fire. (Cron-string parsing is layered on top via
    /// [`crate::scheduler::Scheduler::add_cron`].)
    Every {
        /// The interval between fires, in seconds.
        interval_secs: u64,
        /// The next clock time the job is due.
        next: u64,
    },
}

/// The retry policy: bounded exponential backoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// The first backoff, in seconds.
    pub base_secs: u64,
    /// The multiplier applied each successive failure.
    pub factor: u64,
    /// The cap on a single backoff window, in seconds.
    pub max_secs: u64,
    /// The number of attempts after which the job is marked permanently failed.
    pub max_attempts: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        // A small, fast-growing, bounded schedule: 1s, 2s, 4s, … capped at 5min,
        // five attempts. Conservative enough never to hammer a flaky third party.
        Self {
            base_secs: 1,
            factor: 2,
            max_secs: 300,
            max_attempts: 5,
        }
    }
}

impl RetryPolicy {
    /// The backoff delay for the `attempt`-th retry (1-based), capped at
    /// `max_secs`. Pure arithmetic so the exact retry schedule is assertable.
    #[must_use]
    pub fn backoff_secs(&self, attempt: u32) -> u64 {
        if attempt == 0 {
            return 0;
        }
        let exp = attempt.saturating_sub(1);
        let mult = self.factor.checked_pow(exp).unwrap_or(u64::MAX);
        self.base_secs.saturating_mul(mult).min(self.max_secs)
    }
}

/// The lifecycle state of a persisted job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Waiting for its next due time.
    Scheduled,
    /// Currently executing (a lease the dedup logic respects).
    Running,
    /// Failed but within its retry budget; waiting for the backoff to elapse.
    Retrying,
    /// Permanently failed (retry budget exhausted). Recorded, not dropped.
    Failed,
    /// A `Once` job that completed successfully (terminal).
    Done,
}

/// A persisted unit of scheduled work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    /// Stable id (UUID string).
    pub id: String,
    /// What the job does.
    pub kind: JobKind,
    /// When/how often it fires.
    pub schedule: Schedule,
    /// Retry policy on failure.
    pub retry: RetryPolicy,
    /// Lifecycle state.
    pub state: JobState,
    /// How many times the current run has been attempted (reset on success).
    pub attempts: u32,
    /// When the job becomes eligible to run next, in clock seconds.
    pub due_at: u64,
}

impl Job {
    /// The dedup key for this job (delegates to its kind).
    #[must_use]
    pub fn dedup_key(&self) -> String {
        self.kind.dedup_key()
    }

    /// Whether the job is eligible to run at `now`.
    #[must_use]
    pub fn is_due(&self, now: u64) -> bool {
        matches!(self.state, JobState::Scheduled | JobState::Retrying) && now >= self.due_at
    }
}

/// Persistence seam for jobs. `cellarr-db` can back this; tests use the in-memory
/// store, which also doubles as the "survives a simulated restart" fixture (drop
/// the scheduler, keep the store, build a new scheduler over it).
#[async_trait]
pub trait JobStore: Send + Sync {
    /// The typed error this store reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Insert or replace a job (keyed by [`Job::id`]).
    async fn upsert(&self, job: &Job) -> Result<(), Self::Error>;

    /// Load all persisted jobs.
    async fn load_all(&self) -> Result<Vec<Job>, Self::Error>;

    /// Fetch one job by id.
    async fn get(&self, id: &str) -> Result<Option<Job>, Self::Error>;

    /// Delete a job by id.
    async fn delete(&self, id: &str) -> Result<(), Self::Error>;
}

/// An in-process, thread-safe job store. Persistence here means "outlives the
/// scheduler instance": tests share one across a simulated restart. A real
/// deployment swaps in a `cellarr-db`-backed store with the same interface.
#[derive(Debug, Clone, Default)]
pub struct MemoryJobStore {
    jobs: std::sync::Arc<std::sync::Mutex<HashMap<String, Job>>>,
}

impl MemoryJobStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of persisted jobs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.jobs.lock().expect("job store mutex").len()
    }

    /// Whether the store holds no jobs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The infallible error of the in-memory store.
#[derive(Debug, thiserror::Error)]
#[error("in-memory job store error")]
pub struct MemoryStoreError;

#[async_trait]
impl JobStore for MemoryJobStore {
    type Error = MemoryStoreError;

    async fn upsert(&self, job: &Job) -> Result<(), Self::Error> {
        self.jobs
            .lock()
            .expect("job store mutex")
            .insert(job.id.clone(), job.clone());
        Ok(())
    }

    async fn load_all(&self) -> Result<Vec<Job>, Self::Error> {
        Ok(self
            .jobs
            .lock()
            .expect("job store mutex")
            .values()
            .cloned()
            .collect())
    }

    async fn get(&self, id: &str) -> Result<Option<Job>, Self::Error> {
        Ok(self.jobs.lock().expect("job store mutex").get(id).cloned())
    }

    async fn delete(&self, id: &str) -> Result<(), Self::Error> {
        self.jobs.lock().expect("job store mutex").remove(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_bounded_exponential() {
        let p = RetryPolicy {
            base_secs: 1,
            factor: 2,
            max_secs: 10,
            max_attempts: 6,
        };
        assert_eq!(p.backoff_secs(0), 0);
        assert_eq!(p.backoff_secs(1), 1);
        assert_eq!(p.backoff_secs(2), 2);
        assert_eq!(p.backoff_secs(3), 4);
        assert_eq!(p.backoff_secs(4), 8);
        // Capped at max_secs thereafter.
        assert_eq!(p.backoff_secs(5), 10);
        assert_eq!(p.backoff_secs(6), 10);
    }

    #[test]
    fn dedup_key_distinguishes_manual_searches() {
        let a = JobKind::ManualSearch {
            content_id: "x".into(),
        };
        let b = JobKind::ManualSearch {
            content_id: "y".into(),
        };
        assert_ne!(a.dedup_key(), b.dedup_key());
        assert_eq!(JobKind::RssSync.dedup_key(), JobKind::RssSync.dedup_key());
    }
}
