# Spec: cellarr-migrate

## Responsibility
Import an existing Radarr/Sonarr (and later Lidarr) install into cellarr: library structure +
identity, file associations, quality profiles + custom formats, indexers/clients, history, root
folders/naming/tags. Day-one feature — no migration, no adoption.

## Allowed dependencies
Internal: `cellarr-core`, `cellarr-db`, `cellarr-decide` (CF/profile mapping). External: `sqlx`
(read the source SQLite), `serde`, `thiserror`.

## Public interface
- `preview(source_paths) -> MigrationPreview` — what would be imported, no writes.
- `import(source_paths, &Database) -> MigrationReport` — perform the mapping into the destination
  cellarr DB (the caller owns/opens it; a fresh DB makes the import reversible — throw it away and
  re-import).
- `detect_source(path) -> SourceKind` — Radarr vs Sonarr by schema (Lidarr is recognized and
  reported as not-yet-supported rather than misdetected).
- `recognize::plan_file_operations(install, policy)` — the shared planner that proves the
  recognize-in-place guarantee (zero ops with the in-place policy).

Implemented now: **Radarr** (movies) and **Sonarr** (series/season/episode). Lidarr/music and
history import remain deferred. Indexers/clients/root-folders carry connection settings across
verbatim for re-test on import. Quality-profile cutoffs and allowed qualities map by **quality name**
against `cellarr-core`'s ranking; custom-format `Specifications` route through `cellarr-decide`'s own
TRaSH converter so decisions stay equivalent.

## Behavior
- Open the source DB **read-only**; never mutate the user's existing install (it can keep running).
- Unified result: importing a Radarr DB and a Sonarr DB yields one library set (movies + TV).
- Map TRaSH-style custom formats/profiles via `cellarr-decide` so decisions stay equivalent.
- **Recognize files in place**: migration must never move/delete files; the destructive pipeline only
  runs on future grabs.
- Import is previewable and reversible (throw away the cellarr DB and re-import).

## Test obligations
- Sanitized fixture DBs (schema-representative, no personal data) → asserted schema mapping.
- Round-trip: counts and key identities preserved; imported profiles/CFs reproduce equivalent
  decisions (cross-checked with `corpus/scoring`).
- "Recognize in place": given a library tree + imported DB, **zero** file operations scheduled for
  already-correct files.

## References
[12-migration.md](../12-migration.md), [02-data-model.md](../02-data-model.md),
[08-database.md](../08-database.md).
