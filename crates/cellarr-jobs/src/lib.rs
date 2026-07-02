//! cellarr-jobs — the pipeline executor and scheduler.
//!
//! This crate is the centerpiece that drives `cellarr-core`'s [`Stage`] machine
//! over candidates and schedules the recurring/on-demand work that produces
//! them. It is split into two cooperating halves (see `docs/specs/cellarr-jobs.md`
//! and `docs/03-pipeline.md`):
//!
//! - **The runner** ([`PipelineRunner`]) advances one candidate through
//!   `Discover → Parse → Identify → Decide → Grab → Track → Import → Rename →
//!   Notify`, delegating every type-specific step to a [`cellarr_media`]
//!   [`MediaModule`](cellarr_core::MediaModule) so it never branches on
//!   [`MediaType`](cellarr_core::MediaType). At every transition it appends a
//!   decision-log record (why) and, at grab/terminal points, history (what).
//!   Failure transitions are explicit and logged: reject at Decide, grab-failed
//!   → next release/blocklist, import-failed → hold for review.
//!
//! - **The scheduler** ([`Scheduler`]) registers cron-style recurring jobs (RSS
//!   sync, missing-item search, disk checks) and on-demand jobs; deduplicates
//!   identical in-flight jobs; retries with bounded exponential backoff; caps
//!   per-resource concurrency; and persists jobs (the [`JobStore`] seam) so they
//!   survive a restart. Its time source is a pluggable [`Clock`] so the schedule,
//!   dedup, backoff, and persistence are all tested with a [`LogicalClock`] and
//!   no real sleeps.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod clock;
pub mod error;
pub mod importlists;
pub mod indexers;
pub mod job;
pub mod notify;
pub mod runner;
pub mod scheduler;

pub use clock::{Clock, LogicalClock, SystemClock};
pub use error::{BoxError, JobError, Result};
pub use importlists::{
    DbLibraryIndex, ImportListSync, ImportListSyncError, LibraryIndex, ListSyncReport,
    SourceFactory,
};
pub use indexers::{DbIndexerSet, IndexerSetError, NabAdapter};
pub use job::{
    Job, JobKind, JobState, JobStore, MemoryJobStore, MemoryStoreError, RetryPolicy, Schedule,
};
pub use notify::{ProviderNotifier, WebhookNotifier, WEBHOOK_KIND, WEBHOOK_URL_FIELD};
pub use runner::{
    ManualImportCandidate, ManualImportRequest, ManualImportResult, ManualImportSuggestion,
    PipelineRunner, RescanReport, ReleaseCandidate, RunOutcome, RunnerConfig,
};
pub use scheduler::{ConcurrencyCaps, CronError, JobHandler, JobResult, Scheduler};
