# Spec: cellarr-fs

## Responsibility
All library file operations: scan/inventory, hardlink-or-copy, atomic move, and the rename engine.
**This is the only crate that can destroy user data.** It owns the
stage→verify→commit→log discipline. Treat every change here as safety-critical.

## Allowed dependencies
Internal: `cellarr-core`. External: `tokio` (`spawn_blocking`)/`rayon` for IO/hashing, `serde`,
`thiserror`. A media-info probe for file inspection.

## Public interface
- `scan(root) -> inventory` — discover existing files (for migration "recognize in place" and refresh).
- `plan_import(grab, files, library) -> ImportPlan` — the **Stage** step: full plan, no mutation.
- `execute_import(plan) -> ImportResult` — the **Verify→Commit→Cleanup** steps, crash-safe.
- `rename(content, naming_tokens) -> path` — deterministic on-disk naming from `MediaModule` tokens.
- `hardlink_or_copy(src, dst)` — hardlink within a filesystem; copy+fsync+atomic-rename across.

## Behavior (NON-NEGOTIABLE: library safety)
- **Stage:** compute sources, destinations, replaced files, link-vs-copy, permission/ownership map —
  no filesystem mutation.
- **Verify:** re-parse actual files, confirm match+quality, confirm space/writability, confirm the
  replaced file is genuinely inferior per `cellarr-decide`.
- **Commit:** new file fully in place and durable (fsync) **before** any old file is removed; prefer
  hardlink to preserve the seeding copy; cross-fs uses copy + fsync + atomic rename.
- **Cleanup:** remove replaced files, update DB in one transaction, append history + decision log.
- A crash at any point leaves the library consistent and the operation resumable. Cross-fs move never
  leaves a partial destination. **Never delete the old file before the new file is durable.**
- Inference-derived/low-confidence matches that would replace/delete are held for confirmation
  ([03-pipeline.md](../03-pipeline.md)).

## Test obligations
- **Crash-safety** tests inject failure between each stage and assert consistency + resumability.
- Cross-filesystem move never leaves a partial destination (temp-fs tests).
- Old file never removed before new file durable (ordering test).
- Rename engine: `corpus/naming/*` vectors (content+tokens → expected path), incl. multi-episode
  files mapping to one file and unicode/illegal-character handling per platform.
- Permission/ownership mapping (container UID/GID scenarios) tested.

## References
[03-pipeline.md](../03-pipeline.md), [02-data-model.md](../02-data-model.md),
[11-testing.md](../11-testing.md).
