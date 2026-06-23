# Spec: cellarr-jobs

## Responsibility
Drive the pipeline over time. Owns scheduling (cron-style + on-demand), retries with backoff,
deduplication, per-resource concurrency caps, and execution of the `cellarr-core` pipeline state
machine. Persists jobs so they survive restart.

## Allowed dependencies
Internal: `cellarr-core`, `cellarr-db` (job persistence), and the integration/media/decide/fs crates
it orchestrates (via their traits). External: `tokio`, a scheduler (`apalis` — note: pre-1.0 RC — or
`tokio-cron-scheduler`; decide at scaffold time), `governor`, `serde`, `thiserror`, `tracing`.

## Public interface
- A `Scheduler` that registers recurring jobs (RSS sync, metadata refresh, missing-item search,
  disk-space checks, cleanup) and dispatches on-demand jobs (manual search/import).
- Job submission/cancellation/status APIs (consumed by `cellarr-api`).
- The pipeline runner that advances releases through the state machine, emitting decision-log/history.

## Behavior
- **Deduplicate** identical in-flight jobs (don't run the same search twice concurrently).
- **Per-resource concurrency caps and rate limits** (per indexer/client/host) — coordinate with the
  adapters' own `governor` limits; never stampede a third party.
- Retries use bounded exponential backoff; permanently-failed jobs are recorded, not silently dropped.
- Jobs are persisted; on restart, in-flight work resumes or is safely re-queued.
- Prefer event-driven progress (webhooks) over tight polling where available
  ([03-pipeline.md](../03-pipeline.md)).

## Test obligations
- Scheduling: jobs fire on schedule; on-demand jobs run promptly (logical-clock tests, no real sleeps).
- Dedup: concurrent identical submissions collapse to one run.
- Retry/backoff: failing jobs retry with the expected schedule and eventually mark failed.
- Persistence: jobs survive a simulated restart.
- Concurrency caps: never exceed configured per-resource limits under load.

## References
[03-pipeline.md](../03-pipeline.md), [01-architecture.md](../01-architecture.md).
