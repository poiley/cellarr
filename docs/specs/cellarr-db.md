# Spec: cellarr-db

## Responsibility
The **only** crate that talks to a database. Implements the repository traits from `cellarr-core`
over **SQLite (default)** and **Postgres (opt-in)** via `sqlx`. Owns the schema (migrations), the
**writer-actor**, the cache tables, and FTS. Nothing else writes SQL.

## Allowed dependencies
Internal: `cellarr-core`. External: `sqlx` (sqlite + postgres features), `tokio`, `serde`,
`thiserror`. Postgres support behind a cargo feature.

## Public interface
- Concrete repository types implementing `cellarr-core`'s repository traits (content, files, grabs,
  history, decision-log, profiles, custom-formats, indexers, clients, config, cache).
- A `Database` handle that opens the configured engine, runs migrations, and exposes the repos.
- The **writer handle**: all writes go through a single writer task behind a bounded `mpsc` channel
  (SQLite); on Postgres this is a thin pass-through. Readers use a pool.

## Behavior
- **Schema is authoritative in migrations** (sqlx migrations), forward-only, applied from empty in CI
  on both engines. Docs describe the model; migrations are truth ([08-database.md](../08-database.md)).
- SQLite: WAL mode, nonzero `busy_timeout`, `BEGIN IMMEDIATE` for write txns, short transactions.
- Dialect-specific SQL is isolated behind the repo layer; callers are engine-agnostic.
- Secrets (API keys, client passwords) stored encrypted at rest; never logged.
- Offline `.sqlx` query metadata committed so CI builds without a live DB.

## Test obligations
- Repository tests run against **both** SQLite and Postgres in CI (Postgres via service container).
- Migration test: apply all migrations from empty on both engines, run the suite against the result.
- Writer-actor crash-safety: kill mid-write, reopen, assert consistency.
- FTS search returns expected results for library queries.

## References
[08-database.md](../08-database.md), [02-data-model.md](../02-data-model.md).
