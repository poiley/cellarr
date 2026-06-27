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
- `cellarr managed-config validate [--file PATH]` / `cellarr managed-config export [--file PATH]`:
  the config-as-code surface (see below). `validate` exits `0` clean, `2` on a load/validation error,
  `3` when the file is valid but the live DB has pending drift.
- A second binary target `cellarr-meta` (or a flag) to run `cellarr-meta` standalone.

## Config-as-code (managed config)
- When `managed_config_path` (env `CELLARR_MANAGED_CONFIG_PATH`) points at a declarative YAML file,
  the daemon **reconciles** its DB from that file on boot — after migrations, before serving — so the
  whole operational config can live in git (a k8s ConfigMap). The engine lives in `src/managed/` with
  a **pure** plan step (`managed::plan`) separated from the apply step (`managed::reconcile`).
- The file (`apiVersion: cellarr/v1`) declares, by stable human **name**: `tags`, `rootFolders`,
  `libraries`, `qualityDefinitions`, `customFormats`, `qualityProfiles`, `indexers`, `downloadClients`.
  Shapes mirror the `/api/v3` + core models; indexer/download-client `settings` are a nested map.
- `${ENV}` / `${ENV:-default}` references in string values are interpolated from the process env
  before parsing (so secrets come from k8s Secrets, never committed); a missing required secret is a
  hard error naming the variable. `$$` escapes a literal `$`.
- Reconciliation diffs the declared set against a tracking ledger (`managed_config_entity`, migration
  0017) and creates / updates / prunes via the existing repos. **Prune removes only entities config
  previously managed** — never a UI-created one (which has no ledger row). A section **absent** from
  the file is left untouched; an **empty** section prunes everything config managed for that kind.
  Idempotent: applying the same file twice makes zero changes the second time. A managed-config error
  **fails boot loudly** rather than serving stale/half-applied config.
- Pack-2 deferrals (not yet config-managed): release/delay profiles, import lists, notifications,
  remote-path mappings, naming/media-management, and auth.

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
- Managed config: schema YAML round-trips; secret interpolation (present/missing-required/default/
  literal `$`); the plan computes create/update/prune; idempotency (apply twice ⇒ empty plan); cross-
  reference validation errors; prune removes only config-managed entities (a UI-created one survives);
  export → re-import ⇒ empty plan; a malformed/invalid file fails boot and `validate`.

## References
[01-architecture.md](../01-architecture.md), [08-database.md](../08-database.md),
[12-migration.md](../12-migration.md).
