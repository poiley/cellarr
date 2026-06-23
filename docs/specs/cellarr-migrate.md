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
- `import(source_paths) -> MigrationReport` — perform the mapping into a fresh cellarr DB.
- Source detection (Radarr vs Sonarr vs Lidarr by schema).

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
