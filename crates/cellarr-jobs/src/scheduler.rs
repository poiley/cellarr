//! The scheduler: recurring + on-demand jobs, dedup, backoff, concurrency caps.
//!
//! The scheduler owns *when* and *whether* a job runs; a [`JobHandler`] owns
//! *what* it does (in production, building and driving a [`crate::PipelineRunner`]).
//! Keeping the two apart makes the scheduling logic a pure, logical-clock-driven
//! state machine that tests exercise without sleeps or a live runner.
//!
//! Guarantees, each test-pinned (`docs/specs/cellarr-jobs.md`):
//! - **Recurring jobs fire on schedule**; on-demand jobs run as soon as due.
//! - **Dedup**: two submissions with the same [`Job::dedup_key`] collapse to one
//!   in-flight run — the second is dropped while the first holds the lease.
//! - **Bounded exponential backoff**: a failing job retries on the
//!   [`RetryPolicy`] schedule and, once the budget is exhausted, is recorded
//!   [`JobState::Failed`] (never silently dropped).
//! - **Per-resource concurrency caps**: a configurable maximum of concurrently
//!   running jobs *per resource bucket* is never exceeded.
//! - **Persistence/restart**: every state change is written through the
//!   [`JobStore`], so a fresh scheduler over the same store resumes the schedule.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::clock::Clock;
use crate::job::{Job, JobKind, JobState, JobStore, RetryPolicy, Schedule};

/// The outcome of running one job, reported by a [`JobHandler`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobResult {
    /// The job succeeded.
    Success,
    /// The job failed and should be retried per its policy.
    Retryable {
        /// Why it failed (recorded).
        detail: String,
    },
    /// The job failed in a way no retry can fix (record as failed immediately).
    Permanent {
        /// Why it failed.
        detail: String,
    },
}

/// Executes the work a [`Job`] describes. The production handler builds and runs
/// a [`crate::PipelineRunner`]; tests inject a handler that records calls and
/// returns canned [`JobResult`]s so the scheduling logic is exercised in
/// isolation.
#[async_trait]
pub trait JobHandler: Send + Sync {
    /// Run the job. Must not panic on a job kind it does not handle — return a
    /// [`JobResult::Permanent`] instead, so the scheduler records it.
    async fn handle(&self, kind: &JobKind) -> JobResult;
}

/// A handler injected as a trait object is itself a [`JobHandler`], so the
/// generic [`Scheduler`] can be instantiated with `H = Arc<dyn JobHandler>`.
///
/// This is the seam the daemon uses to swap a concrete pipeline handler in
/// behind the same scheduler type the API constructs by default: callers hold a
/// `Scheduler<C, S, Arc<dyn JobHandler>>` and inject whichever handler they
/// built (the event-only default, or the live pipeline handler) without the
/// scheduler's type changing.
#[async_trait]
impl JobHandler for Arc<dyn JobHandler> {
    async fn handle(&self, kind: &JobKind) -> JobResult {
        (**self).handle(kind).await
    }
}

/// Tracks per-resource in-flight counts to enforce concurrency caps and which
/// dedup keys are currently leased (running) to enforce deduplication.
#[derive(Debug, Default)]
struct InFlight {
    /// resource bucket -> count of currently-running jobs.
    per_resource: HashMap<&'static str, u32>,
    /// dedup key -> the job id holding the lease.
    leased_keys: HashMap<String, String>,
}

/// Configuration for the scheduler's concurrency caps.
#[derive(Debug, Clone)]
pub struct ConcurrencyCaps {
    /// Per-resource-bucket maximum of concurrently running jobs. Buckets absent
    /// from the map fall back to `default_cap`.
    pub per_resource: HashMap<&'static str, u32>,
    /// The cap applied to any resource bucket not named in `per_resource`.
    pub default_cap: u32,
}

impl Default for ConcurrencyCaps {
    fn default() -> Self {
        // Conservative: one concurrent indexer job by default (indexers ban
        // aggressively), a little more headroom elsewhere.
        let mut per_resource = HashMap::new();
        per_resource.insert("indexer", 1u32);
        Self {
            per_resource,
            default_cap: 2,
        }
    }
}

impl ConcurrencyCaps {
    fn cap_for(&self, resource: &'static str) -> u32 {
        self.per_resource
            .get(resource)
            .copied()
            .unwrap_or(self.default_cap)
    }
}

/// The scheduler.
///
/// Generic over its [`Clock`], [`JobStore`], and [`JobHandler`] so production
/// wiring and tests share one implementation. It is driven by [`Scheduler::tick`]:
/// each tick runs every job that is due at the current clock time, respecting
/// dedup and concurrency caps. Production calls `tick` from a `tokio::time`
/// interval; tests call it directly after advancing a [`crate::clock::LogicalClock`].
pub struct Scheduler<C, S, H>
where
    C: Clock,
    S: JobStore,
    H: JobHandler,
{
    clock: Arc<C>,
    store: Arc<S>,
    handler: Arc<H>,
    caps: ConcurrencyCaps,
    in_flight: Arc<tokio::sync::Mutex<InFlight>>,
}

impl<C, S, H> Scheduler<C, S, H>
where
    C: Clock + 'static,
    S: JobStore + 'static,
    H: JobHandler + 'static,
{
    /// Build a scheduler over its seams.
    pub fn new(clock: Arc<C>, store: Arc<S>, handler: Arc<H>, caps: ConcurrencyCaps) -> Self {
        Self {
            clock,
            store,
            handler,
            caps,
            in_flight: Arc::new(tokio::sync::Mutex::new(InFlight::default())),
        }
    }

    /// The job store, for reading state in tests/callers.
    #[must_use]
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// Register a recurring job firing every `interval_secs`, first due `now`.
    ///
    /// Recurring registrations are themselves deduplicated by dedup key: adding
    /// the same recurring kind twice keeps the existing schedule. Returns the
    /// job id.
    ///
    /// # Errors
    /// Propagates store errors.
    pub async fn add_recurring(
        &self,
        kind: JobKind,
        interval_secs: u64,
        retry: RetryPolicy,
    ) -> Result<String, S::Error> {
        let now = self.clock.now_secs();
        if let Some(existing) = self.find_by_dedup(&kind.dedup_key()).await? {
            return Ok(existing.id);
        }
        let job = Job {
            id: new_id(),
            schedule: Schedule::Every {
                interval_secs,
                next: now,
            },
            retry,
            state: JobState::Scheduled,
            attempts: 0,
            due_at: now,
            kind,
        };
        self.store.upsert(&job).await?;
        Ok(job.id)
    }

    /// Register a recurring job from a cron-style expression.
    ///
    /// To keep scheduling deterministic and logical-clock-testable, a cron
    /// expression is reduced to its fixed *period*: the common `@hourly` /
    /// `@daily` macros and a small `*/N * * * *` (every N minutes) form are
    /// recognized; anything else is rejected so a silently-misparsed schedule
    /// can't slip through.
    ///
    /// # Errors
    /// [`CronError`] if the expression is not a supported form; store errors
    /// otherwise.
    pub async fn add_cron(
        &self,
        kind: JobKind,
        cron: &str,
        retry: RetryPolicy,
    ) -> Result<String, CronError<S::Error>> {
        let interval = parse_cron_interval(cron).ok_or_else(|| CronError::Unsupported {
            expr: cron.to_string(),
        })?;
        self.add_recurring(kind, interval, retry)
            .await
            .map_err(CronError::Store)
    }

    /// Submit an on-demand job, due immediately.
    ///
    /// Deduplicated: if an identical job (same dedup key) is already scheduled or
    /// running, returns its id without enqueuing a duplicate.
    ///
    /// # Errors
    /// Propagates store errors.
    pub async fn submit_now(&self, kind: JobKind, retry: RetryPolicy) -> Result<String, S::Error> {
        let now = self.clock.now_secs();
        if let Some(existing) = self.find_active_by_dedup(&kind.dedup_key()).await? {
            return Ok(existing.id);
        }
        let job = Job {
            id: new_id(),
            schedule: Schedule::Once { at: now },
            retry,
            state: JobState::Scheduled,
            attempts: 0,
            due_at: now,
            kind,
        };
        self.store.upsert(&job).await?;
        Ok(job.id)
    }

    /// Cancel a job (remove it from the store). A running job's in-flight lease
    /// is released when it finishes; cancellation prevents future scheduling.
    ///
    /// # Errors
    /// Propagates store errors.
    pub async fn cancel(&self, id: &str) -> Result<(), S::Error> {
        self.store.delete(id).await
    }

    /// Run every job that is due at the current clock time.
    ///
    /// Returns the number of jobs that were actually dispatched this tick (i.e.
    /// not skipped by dedup or a concurrency cap). Jobs are run sequentially
    /// within a tick for determinism; concurrency *across* ticks is what the caps
    /// bound, which is what tests assert by interleaving ticks.
    ///
    /// # Errors
    /// Propagates store errors (a handler's failure is recorded on the job, not
    /// returned).
    pub async fn tick(&self) -> Result<usize, S::Error> {
        let now = self.clock.now_secs();
        let mut jobs = self.store.load_all().await?;
        // Deterministic order: by due time then id, so logical-clock tests are
        // reproducible.
        jobs.sort_by(|a, b| a.due_at.cmp(&b.due_at).then_with(|| a.id.cmp(&b.id)));

        let mut dispatched = 0;
        for job in jobs {
            if !job.is_due(now) {
                continue;
            }
            if self.try_dispatch(job).await? {
                dispatched += 1;
            }
        }
        Ok(dispatched)
    }

    /// Attempt to run one due job, respecting dedup and the concurrency cap.
    /// Returns `true` if it ran, `false` if skipped.
    async fn try_dispatch(&self, mut job: Job) -> Result<bool, S::Error> {
        let resource = job.kind.resource();
        let dedup_key = job.dedup_key();

        // --- Acquire the lease (dedup + concurrency cap) ------------------
        {
            let mut guard = self.in_flight.lock().await;
            // Dedup: an identical job already running blocks this one.
            if guard.leased_keys.contains_key(&dedup_key) {
                return Ok(false);
            }
            // Concurrency cap for the resource bucket.
            let count = guard.per_resource.get(resource).copied().unwrap_or(0);
            if count >= self.caps.cap_for(resource) {
                return Ok(false);
            }
            guard.leased_keys.insert(dedup_key.clone(), job.id.clone());
            *guard.per_resource.entry(resource).or_insert(0) += 1;
        }

        // Mark running and persist (so a restart sees the lease).
        job.state = JobState::Running;
        self.store.upsert(&job).await?;

        // --- Execute -----------------------------------------------------
        let result = self.handler.handle(&job.kind).await;

        // --- Release the lease -------------------------------------------
        {
            let mut guard = self.in_flight.lock().await;
            guard.leased_keys.remove(&dedup_key);
            if let Some(c) = guard.per_resource.get_mut(resource) {
                *c = c.saturating_sub(1);
            }
        }

        // --- Apply the result to the job's lifecycle ---------------------
        self.apply_result(&mut job, result).await?;
        Ok(true)
    }

    /// Transition a job after a run completes, scheduling the next fire or a
    /// backoff retry, and persisting the new state.
    async fn apply_result(&self, job: &mut Job, result: JobResult) -> Result<(), S::Error> {
        let now = self.clock.now_secs();
        match result {
            JobResult::Success => {
                job.attempts = 0;
                match job.schedule {
                    Schedule::Once { .. } => {
                        job.state = JobState::Done;
                        // Terminal on-demand jobs are kept (Done) so callers can
                        // observe the outcome; a sweeper can prune them.
                        self.store.upsert(job).await?;
                    }
                    Schedule::Every { interval_secs, .. } => {
                        let next = now.saturating_add(interval_secs);
                        job.schedule = Schedule::Every {
                            interval_secs,
                            next,
                        };
                        job.due_at = next;
                        job.state = JobState::Scheduled;
                        self.store.upsert(job).await?;
                    }
                }
            }
            JobResult::Retryable { detail } => {
                job.attempts = job.attempts.saturating_add(1);
                if job.attempts >= job.retry.max_attempts {
                    job.state = JobState::Failed;
                    tracing::warn!(job_id = %job.id, attempts = job.attempts, %detail, "job permanently failed after retries");
                } else {
                    let backoff = job.retry.backoff_secs(job.attempts);
                    job.due_at = now.saturating_add(backoff);
                    job.state = JobState::Retrying;
                    tracing::info!(job_id = %job.id, attempt = job.attempts, backoff_secs = backoff, %detail, "job will retry");
                }
                self.store.upsert(job).await?;
            }
            JobResult::Permanent { detail } => {
                job.state = JobState::Failed;
                tracing::warn!(job_id = %job.id, %detail, "job permanently failed (non-retryable)");
                self.store.upsert(job).await?;
            }
        }
        Ok(())
    }

    async fn find_by_dedup(&self, key: &str) -> Result<Option<Job>, S::Error> {
        Ok(self
            .store
            .load_all()
            .await?
            .into_iter()
            .find(|j| j.dedup_key() == key))
    }

    /// Find a job with `key` that is still active (scheduled/running/retrying),
    /// used to dedup on-demand submissions against in-flight or pending work.
    async fn find_active_by_dedup(&self, key: &str) -> Result<Option<Job>, S::Error> {
        Ok(self.store.load_all().await?.into_iter().find(|j| {
            j.dedup_key() == key
                && matches!(
                    j.state,
                    JobState::Scheduled | JobState::Running | JobState::Retrying
                )
        }))
    }
}

/// Errors from cron registration.
#[derive(Debug, thiserror::Error)]
pub enum CronError<E: std::error::Error + Send + Sync + 'static> {
    /// The expression is not one of the supported deterministic forms.
    #[error("unsupported cron expression: {expr}")]
    Unsupported {
        /// The offending expression.
        expr: String,
    },
    /// The underlying job store failed.
    #[error(transparent)]
    Store(E),
}

/// Reduce a small set of cron expressions to a fixed interval in seconds.
///
/// Supported: `@hourly`, `@daily`, `@weekly`, and `*/N * * * *` (every N
/// minutes). This deliberately covers the recurring jobs the spec names (RSS
/// sync, missing-item search, disk checks) without pulling a full cron evaluator
/// onto the deterministic scheduling path.
fn parse_cron_interval(cron: &str) -> Option<u64> {
    match cron.trim() {
        "@hourly" => Some(3600),
        "@daily" | "@midnight" => Some(86_400),
        "@weekly" => Some(604_800),
        other => {
            // `*/N * * * *` — every N minutes.
            let fields: Vec<&str> = other.split_whitespace().collect();
            if fields.len() == 5 && fields[1..] == ["*", "*", "*", "*"] {
                if let Some(rest) = fields[0].strip_prefix("*/") {
                    if let Ok(n) = rest.parse::<u64>() {
                        if n > 0 {
                            return Some(n.saturating_mul(60));
                        }
                    }
                }
            }
            None
        }
    }
}

/// A fresh job id.
fn new_id() -> String {
    cellarr_core::PipelineRunId::new().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_macros_and_every_n_minutes() {
        assert_eq!(parse_cron_interval("@hourly"), Some(3600));
        assert_eq!(parse_cron_interval("@daily"), Some(86_400));
        assert_eq!(parse_cron_interval("*/15 * * * *"), Some(900));
        assert_eq!(parse_cron_interval("*/1 * * * *"), Some(60));
        assert_eq!(parse_cron_interval("0 0 * * *"), None);
        assert_eq!(parse_cron_interval("garbage"), None);
    }
}
