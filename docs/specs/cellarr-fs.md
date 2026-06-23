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
- `check_same_filesystem(downloads_dir, library_roots) -> Vec<FilesystemWarning>` — the loud
  cross-filesystem (silent-copy-fallback) health check. See **Same-filesystem (`st_dev`) warning**
  below.

## Same-filesystem (`st_dev`) detection + the loud cross-filesystem warning (the differentiator)
The single biggest silent footgun in a Sonarr/Radarr-style stack is a **downloads directory on a
different filesystem than the library**:

- **Same filesystem:** an import is an instant **hardlink** — no extra disk, and the seeding copy the
  torrent client is still serving is preserved (two names, one inode). This is the device test
  (`st_dev` of source vs destination via `same_filesystem`); the import planner records the truth on
  each `PlannedMove::hardlink` so Commit never guesses.
- **Different filesystems:** a hardlink is impossible, so `hardlink_or_copy` falls back to a full
  **copy + fsync + atomic rename**. That fallback is always *correct*, but it silently doubles disk
  use per import and breaks the preserve-the-seeding-copy property. The originals do this fallback
  **silently**; users discover it only when a disk fills or seeding breaks.

cellarr makes it **loud**. `check_same_filesystem` compares the configured downloads directory's
`st_dev` against every library root and returns one `FilesystemWarning::CrossFilesystem { downloads_dir,
library_root }` per off-device root. A configured-but-not-yet-created root resolves to its nearest
existing ancestor before `stat`; a path with no existing ancestor is skipped (a missing optional path
must never take down the whole health read); a genuine `stat` failure (e.g. an unreadable downloads
dir) is surfaced as an error. On non-unix the device id is unknowable, so the check compares the root
component rather than spamming false warnings (import correctness never depends on this — only the
warning's precision).

The warning is surfaced by `cellarr-api` on **both faces** of `/api/v3/health` (the Sonarr/Radarr
shim, as a `{ source: "ImportMechanismCheck", type: "warning", message, wikiUrl }` record) **and** in
the native system-health snapshot, and is `warn!`-logged on every observation
(`cellarr_api::fs_health::filesystem_warnings`). The downloads dir is read from each enabled download
client's `settings` JSON (`download_dir`/`downloadDir`/`save_path`); a client with no configured
downloads dir is skipped rather than guessed.

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
