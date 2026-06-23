# 08 — Database

## Policy: SQLite by default, Postgres by opt-in

`cellarr-db` is the only crate that talks to the database. It exposes **repository traits** to the
rest of the system; nothing else writes SQL. The engine is chosen at startup from config.

- **Default: SQLite in WAL mode.** Zero-config, single file, embedded — exactly the self-host
  profile, and what the originals default to. Handles millions of rows for this read-heavy,
  low-write-rate workload.
- **Opt-in: Postgres**, via a connection string, for power users (huge libraries, multiple
  instances, Kubernetes/networked storage).

We use **`sqlx`** so one codebase serves both, with compile-time-checked queries (offline
`.sqlx`/`sqlx prepare` artifacts committed so CI builds without a live DB). Budget for maintaining
two SQL dialects; keep dialect-specific SQL isolated behind the repository layer.

## The single-writer reality (and how we tame it)

SQLite allows exactly one writer at a time (one WAL, no row-level write locks). We make this
explicit instead of fighting it:

- **All writes funnel through one writer task** behind a bounded `mpsc` channel. Reads use a
  connection pool. This removes scattered `SQLITE_BUSY` handling and a class of races.
- Set a **nonzero `busy_timeout`**, use **`BEGIN IMMEDIATE`** for known-write transactions (a
  deferred txn that upgrades read→write mid-flight errors *without* invoking the busy handler), and
  keep transactions short.
- WAL creates `-wal`/`-shm` side files and **does not work over network filesystems** (the
  wal-index is shared memory; all access must be same-host). This is *the* reason a user would pick
  Postgres — document it.

For Postgres, the writer-actor is unnecessary (MVCC), but the repository interface is identical, so
the rest of cellarr is unaffected by the choice.

## Schema ownership & migrations

- The **authoritative schema lives in versioned migrations** in `cellarr-db` (sqlx migrations).
  Docs describe the model ([02-data-model.md](02-data-model.md)); migrations are the source of truth.
- Migrations are **forward-only and tested**: a CI job applies every migration from empty on both
  engines and runs the repository test suite against the result.
- Schema changes that affect the data model require agreement (see the agent guide) because every
  crate builds on it.

## What we deliberately do NOT use

- **No Redis** — in-process `moka` caches and the DB cover caching. Adding Redis violates the
  zero-required-services non-negotiable.
- **No Elasticsearch** — UI/library search uses **SQLite FTS5** (or Postgres full-text). If we ever
  outgrow it, `tantivy` (embedded) is the escalation, not a separate service.
- **No external job broker** — `cellarr-jobs` persists jobs in the DB.

## Migration from existing *arr databases

Importing a user's existing Radarr/Sonarr SQLite DB is a **read** of their file into cellarr's
schema, handled by `cellarr-migrate` ([12-migration.md](12-migration.md)). Note the originals chose
"new installs only" for their own SQLite→Postgres migration; cellarr's importer is a different
thing (cross-app import) and is a first-class day-one feature.

## Testing

- Repository tests run against **both** engines in CI (SQLite always; Postgres via a service
  container).
- Crash-safety of the writer-actor and transaction boundaries are tested (kill mid-write, reopen,
  assert consistency) — this underpins the library-safety non-negotiable together with
  [`specs/cellarr-fs.md`](specs/cellarr-fs.md).

See [`specs/cellarr-db.md`](specs/cellarr-db.md).
