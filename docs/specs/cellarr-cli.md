# Spec: cellarr-cli

## Responsibility
The daemon **binary**. The only place wiring happens: parse args/config, open the database, build the
metadata/indexer/client/media registries, start the scheduler and the API server, and run. Also
provides operator subcommands (migrate, config check, one-off tasks). May depend on everything; no
crate depends on it.

## Allowed dependencies
Internal: all crates. External: `tokio`, `figment` (layered config), an args parser (`clap`),
`tracing-subscriber`, `anyhow` (top-level error handling).

## Public interface
- `cellarr` (run the daemon) and subcommands: `migrate`, `config check`, `task <name>`, `version`.
- A second binary target `cellarr-meta` (or a flag) to run `cellarr-meta` standalone.

## Behavior
- **Zero-config startup:** runs out of the box with sensible defaults (SQLite in the data dir),
  satisfying the single-binary/offline non-negotiables.
- Config precedence: defaults → file → environment.
- Graceful shutdown: drain the writer-actor, finish/park in-flight jobs, close connections.
- Wires optional features (Postgres, plugins, llm providers) per config/build features without making
  them required.
- Structured logging via `tracing`; optional metrics/OTLP, never required.

## Test obligations
- Boots with empty config to a working daemon (smoke test) and serves a health endpoint.
- Config layering resolves as specified (defaults < file < env).
- Graceful shutdown leaves a consistent DB (no torn writes).
- `migrate` subcommand drives `cellarr-migrate` end to end on a fixture DB.

## References
[01-architecture.md](../01-architecture.md), [08-database.md](../08-database.md),
[12-migration.md](../12-migration.md).
