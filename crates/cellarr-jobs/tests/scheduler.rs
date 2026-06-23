//! Scheduler behaviour tests, driven entirely by a logical clock (no real
//! sleeps). Each test name states the spec property it pins.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use cellarr_jobs::clock::LogicalClock;
use cellarr_jobs::job::{JobKind, JobState, MemoryJobStore, RetryPolicy, Schedule};
use cellarr_jobs::scheduler::{ConcurrencyCaps, JobHandler, JobResult, Scheduler};
use cellarr_jobs::JobStore;

/// A handler that records the kinds it was asked to run and returns a canned
/// result, so the scheduling state machine is exercised without a live runner.
struct RecordingHandler {
    calls: Mutex<Vec<JobKind>>,
    result: Mutex<JobResult>,
    count: AtomicU32,
}

impl RecordingHandler {
    fn new(result: JobResult) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            result: Mutex::new(result),
            count: AtomicU32::new(0),
        })
    }
    fn call_count(&self) -> u32 {
        self.count.load(Ordering::SeqCst)
    }
    fn set_result(&self, r: JobResult) {
        *self.result.lock().unwrap() = r;
    }
}

#[async_trait]
impl JobHandler for RecordingHandler {
    async fn handle(&self, kind: &JobKind) -> JobResult {
        self.calls.lock().unwrap().push(kind.clone());
        self.count.fetch_add(1, Ordering::SeqCst);
        self.result.lock().unwrap().clone()
    }
}

fn caps(cap: u32) -> ConcurrencyCaps {
    let mut per = std::collections::HashMap::new();
    per.insert("indexer", cap);
    ConcurrencyCaps {
        per_resource: per,
        default_cap: cap,
    }
}

#[tokio::test]
async fn recurring_job_fires_once_per_interval_on_schedule() {
    let clock = Arc::new(LogicalClock::new(0));
    let store = Arc::new(MemoryJobStore::new());
    let handler = RecordingHandler::new(JobResult::Success);
    let sched = Scheduler::new(clock.clone(), store.clone(), handler.clone(), caps(4));

    // Every 60s, first due at t=0.
    sched
        .add_recurring(JobKind::RssSync, 60, RetryPolicy::default())
        .await
        .unwrap();

    // t=0: due, fires.
    assert_eq!(sched.tick().await.unwrap(), 1);
    assert_eq!(handler.call_count(), 1);

    // t=30: not yet due again.
    clock.set(30);
    assert_eq!(sched.tick().await.unwrap(), 0);
    assert_eq!(handler.call_count(), 1);

    // t=60: due again.
    clock.set(60);
    assert_eq!(sched.tick().await.unwrap(), 1);
    assert_eq!(handler.call_count(), 2);

    // t=125: one more interval elapsed -> exactly one more fire.
    clock.set(125);
    assert_eq!(sched.tick().await.unwrap(), 1);
    assert_eq!(handler.call_count(), 3);
}

#[tokio::test]
async fn on_demand_job_runs_promptly_then_is_done() {
    let clock = Arc::new(LogicalClock::new(1000));
    let store = Arc::new(MemoryJobStore::new());
    let handler = RecordingHandler::new(JobResult::Success);
    let sched = Scheduler::new(clock.clone(), store.clone(), handler.clone(), caps(4));

    let id = sched
        .submit_now(
            JobKind::ManualSearch {
                content_id: "abc".into(),
            },
            RetryPolicy::default(),
        )
        .await
        .unwrap();

    assert_eq!(sched.tick().await.unwrap(), 1);
    assert_eq!(handler.call_count(), 1);

    let job = store.get(&id).await.unwrap().unwrap();
    assert_eq!(job.state, JobState::Done);

    // A Done one-shot does not run again on subsequent ticks.
    assert_eq!(sched.tick().await.unwrap(), 0);
}

#[tokio::test]
async fn identical_in_flight_jobs_are_deduplicated() {
    let clock = Arc::new(LogicalClock::new(0));
    let store = Arc::new(MemoryJobStore::new());
    let handler = RecordingHandler::new(JobResult::Success);
    let sched = Scheduler::new(clock.clone(), store.clone(), handler.clone(), caps(4));

    // Two submissions of the same logical job collapse to one persisted job.
    let id1 = sched
        .submit_now(JobKind::RssSync, RetryPolicy::default())
        .await
        .unwrap();
    let id2 = sched
        .submit_now(JobKind::RssSync, RetryPolicy::default())
        .await
        .unwrap();
    assert_eq!(id1, id2, "duplicate submission must return the same job id");
    assert_eq!(store.load_all().await.unwrap().len(), 1);

    assert_eq!(sched.tick().await.unwrap(), 1);
    assert_eq!(handler.call_count(), 1);
}

#[tokio::test]
async fn failing_job_retries_with_bounded_exponential_backoff_then_fails() {
    let clock = Arc::new(LogicalClock::new(0));
    let store = Arc::new(MemoryJobStore::new());
    let handler = RecordingHandler::new(JobResult::Retryable {
        detail: "boom".into(),
    });
    let policy = RetryPolicy {
        base_secs: 1,
        factor: 2,
        max_secs: 100,
        max_attempts: 4,
    };
    let sched = Scheduler::new(clock.clone(), store.clone(), handler.clone(), caps(4));

    let id = sched
        .submit_now(
            JobKind::ManualSearch {
                content_id: "x".into(),
            },
            policy,
        )
        .await
        .unwrap();

    // Attempt 1 at t=0 -> backoff 1s, due at t=1, state Retrying.
    assert_eq!(sched.tick().await.unwrap(), 1);
    let job = store.get(&id).await.unwrap().unwrap();
    assert_eq!(job.attempts, 1);
    assert_eq!(job.state, JobState::Retrying);
    assert_eq!(job.due_at, 1);

    // Not due before the backoff elapses.
    assert_eq!(sched.tick().await.unwrap(), 0);

    // t=1: attempt 2 -> backoff 2s -> due t=3.
    clock.set(1);
    assert_eq!(sched.tick().await.unwrap(), 1);
    assert_eq!(store.get(&id).await.unwrap().unwrap().due_at, 3);

    // t=3: attempt 3 -> backoff 4s -> due t=7.
    clock.set(3);
    assert_eq!(sched.tick().await.unwrap(), 1);
    assert_eq!(store.get(&id).await.unwrap().unwrap().due_at, 7);

    // t=7: attempt 4 == max_attempts -> permanently Failed (recorded, not dropped).
    clock.set(7);
    assert_eq!(sched.tick().await.unwrap(), 1);
    let job = store.get(&id).await.unwrap().unwrap();
    assert_eq!(job.attempts, 4);
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(handler.call_count(), 4);

    // A failed job is not rescheduled.
    clock.set(1000);
    assert_eq!(sched.tick().await.unwrap(), 0);
}

#[tokio::test]
async fn retry_then_success_resets_attempts() {
    let clock = Arc::new(LogicalClock::new(0));
    let store = Arc::new(MemoryJobStore::new());
    let handler = RecordingHandler::new(JobResult::Retryable {
        detail: "transient".into(),
    });
    let sched = Scheduler::new(clock.clone(), store.clone(), handler.clone(), caps(4));

    let id = sched
        .add_recurring(JobKind::MissingItemSearch, 3600, RetryPolicy::default())
        .await
        .unwrap();

    // First fire fails -> retrying.
    assert_eq!(sched.tick().await.unwrap(), 1);
    assert_eq!(
        store.get(&id).await.unwrap().unwrap().state,
        JobState::Retrying
    );

    // The transient condition clears.
    handler.set_result(JobResult::Success);
    let due = store.get(&id).await.unwrap().unwrap().due_at;
    clock.set(due);
    assert_eq!(sched.tick().await.unwrap(), 1);
    let job = store.get(&id).await.unwrap().unwrap();
    assert_eq!(job.attempts, 0);
    assert_eq!(job.state, JobState::Scheduled);
    // Recurring: rescheduled one interval out.
    assert_eq!(job.due_at, due + 3600);
}

#[tokio::test]
async fn per_resource_concurrency_cap_is_never_exceeded() {
    // A handler that records the max number of concurrent in-flight calls.
    struct ConcurrencyProbe {
        active: AtomicU32,
        max_seen: AtomicU32,
        gate: tokio::sync::Semaphore,
    }
    #[async_trait]
    impl JobHandler for ConcurrencyProbe {
        async fn handle(&self, _kind: &JobKind) -> JobResult {
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_seen.fetch_max(now, Ordering::SeqCst);
            // Hold the slot until released, to force overlap if the cap allowed it.
            let _permit = self.gate.acquire().await.unwrap();
            self.active.fetch_sub(1, Ordering::SeqCst);
            JobResult::Success
        }
    }

    let clock = Arc::new(LogicalClock::new(0));
    let store = Arc::new(MemoryJobStore::new());
    let probe = Arc::new(ConcurrencyProbe {
        active: AtomicU32::new(0),
        max_seen: AtomicU32::new(0),
        gate: tokio::sync::Semaphore::new(0),
    });
    // Cap the "indexer" bucket at 1 concurrent run.
    let sched = Arc::new(Scheduler::new(
        clock.clone(),
        store.clone(),
        probe.clone(),
        caps(1),
    ));

    // Two distinct indexer-bucket jobs, both due now.
    sched
        .submit_now(
            JobKind::ManualSearch {
                content_id: "a".into(),
            },
            RetryPolicy::default(),
        )
        .await
        .unwrap();
    sched
        .submit_now(
            JobKind::ManualSearch {
                content_id: "b".into(),
            },
            RetryPolicy::default(),
        )
        .await
        .unwrap();

    // Run two ticks concurrently; the cap must serialize them.
    let s1 = sched.clone();
    let s2 = sched.clone();
    let t1 = tokio::spawn(async move { s1.tick().await });
    let t2 = tokio::spawn(async move { s2.tick().await });

    // Give both ticks a moment to reach the gate, then release one at a time.
    tokio::task::yield_now().await;
    // Release enough permits for both handler bodies to finish.
    probe.gate.add_permits(2);

    let r1 = t1.await.unwrap().unwrap();
    let r2 = t2.await.unwrap().unwrap();
    assert_eq!(r1 + r2, 2, "both jobs eventually dispatch");
    assert_eq!(
        probe.max_seen.load(Ordering::SeqCst),
        1,
        "never more than one indexer job ran at once"
    );
}

#[tokio::test]
async fn jobs_survive_a_simulated_restart() {
    let clock = Arc::new(LogicalClock::new(0));
    let store = Arc::new(MemoryJobStore::new());

    // First scheduler instance registers a recurring job and a failing one mid-retry.
    {
        let handler = RecordingHandler::new(JobResult::Retryable {
            detail: "down".into(),
        });
        let sched = Scheduler::new(clock.clone(), store.clone(), handler.clone(), caps(4));
        sched
            .add_recurring(JobKind::RssSync, 600, RetryPolicy::default())
            .await
            .unwrap();
        let _id = sched
            .submit_now(
                JobKind::ManualSearch {
                    content_id: "z".into(),
                },
                RetryPolicy::default(),
            )
            .await
            .unwrap();
        // Fire both; the manual one goes Retrying, the rss one reschedules.
        sched.tick().await.unwrap();
        // scheduler dropped here -> "process exit"
    }

    // Both jobs are still in the (persistent) store.
    let persisted = store.load_all().await.unwrap();
    assert_eq!(persisted.len(), 2);

    // A brand new scheduler over the same store resumes the schedules.
    let handler2 = RecordingHandler::new(JobResult::Success);
    let sched2 = Scheduler::new(clock.clone(), store.clone(), handler2.clone(), caps(4));

    // Advance the clock well past every due time; the resumed scheduler runs the
    // pending work without any re-registration.
    clock.set(100_000);
    let dispatched = sched2.tick().await.unwrap();
    assert!(dispatched >= 1, "resumed scheduler runs persisted due jobs");
    assert!(handler2.call_count() >= 1);
}

#[tokio::test]
async fn cancelling_a_job_prevents_future_runs() {
    let clock = Arc::new(LogicalClock::new(0));
    let store = Arc::new(MemoryJobStore::new());
    let handler = RecordingHandler::new(JobResult::Success);
    let sched = Scheduler::new(clock.clone(), store.clone(), handler.clone(), caps(4));

    let id = sched
        .add_recurring(JobKind::DiskSpaceCheck, 60, RetryPolicy::default())
        .await
        .unwrap();
    sched.cancel(&id).await.unwrap();
    assert!(store.get(&id).await.unwrap().is_none());

    clock.set(120);
    assert_eq!(sched.tick().await.unwrap(), 0);
    assert_eq!(handler.call_count(), 0);
}

#[tokio::test]
async fn add_cron_reduces_macros_and_minute_intervals() {
    let clock = Arc::new(LogicalClock::new(0));
    let store = Arc::new(MemoryJobStore::new());
    let handler = RecordingHandler::new(JobResult::Success);
    let sched = Scheduler::new(clock.clone(), store.clone(), handler.clone(), caps(4));

    let id = sched
        .add_cron(JobKind::RssSync, "*/15 * * * *", RetryPolicy::default())
        .await
        .unwrap();
    let job = store.get(&id).await.unwrap().unwrap();
    match job.schedule {
        Schedule::Every { interval_secs, .. } => assert_eq!(interval_secs, 900),
        Schedule::Once { .. } => panic!("cron must produce a recurring schedule"),
    }

    // An unsupported expression is rejected, not silently misparsed.
    let err = sched
        .add_cron(JobKind::RssSync, "0 0 1 * *", RetryPolicy::default())
        .await;
    assert!(err.is_err());
}
