# 03 — The acquisition pipeline

One state machine drives every acquisition, for every media type. It lives in `cellarr-core`
(types + transition logic) and is executed by `cellarr-jobs`. It is **media-type-agnostic**: it
carries a `ContentRef` ([02-data-model.md](02-data-model.md)) and delegates all type-specific
behavior to the `MediaModule`.

## The stages

```
Discover → Parse → Identify → Decide → Grab → Track → Import → Rename → Notify
```

| Stage | Input | Output | Crate |
|-------|-------|--------|-------|
| **Discover** | a need (RSS tick, manual search, missing-item scan) | candidate releases (raw titles + indexer metadata) | `cellarr-indexers` |
| **Parse** | a release *title* string | `ParsedRelease` with per-field confidence | `cellarr-parse` |
| **Identify** | `ParsedRelease` | `Vec<ContentMatch>` (which content node(s), confidence) | `cellarr-media` |
| **Decide** | matched release + profile | grab / upgrade / reject + reason | `cellarr-decide` |
| **Grab** | chosen release | a `grab` row + handoff to a download client | `cellarr-download` |
| **Track** | a `grab` | completion (poll or webhook) | `cellarr-download` |
| **Import** | completed download | files placed in the library | `cellarr-fs` |
| **Rename** | imported files | final on-disk names | `cellarr-fs` |
| **Notify** | terminal outcome | user notifications + UI push | `cellarr-api` + notifiers |

## Two parses, not one

A subtle but critical correctness rule borrowed from the originals:

1. **Parse the release title** at Discover time to decide whether to grab. Titles are
   advertising — they lie, they're truncated, they're mislabeled.
2. **Re-parse the actual file(s)** at Import time and re-verify the match before touching the
   library. The file is the source of truth. If the file parse disagrees with the grab's intent
   beyond tolerance, the import is held for review, never force-fit.

## Discover: event-driven, not poll-only

The originals are poll-heavy. cellarr prefers events where possible (download-client webhooks,
push from a Prowlarr-style sync) and falls back to scheduled RSS sync via `cellarr-jobs`. RSS sync
still exists — it's how new releases are noticed — but status tracking should prefer webhooks over
tight polling loops. Per-indexer rate limits (`governor`) apply to all Discover traffic.

## Decide: and write down why

Every decision appends to the **`decision_log`**: the release considered, its parsed fields, its
computed quality and custom-format score, the comparison against what's on disk, and the verdict
(grabbed / rejected-because / upgraded-over). This table is:

- the answer to the #1 user question ("why did it grab/delete/ignore that?"),
- a replay-for-debug trail,
- and a test surface (the differential oracle compares decisions, see [11-testing.md](11-testing.md)).

We use a **pragmatic** version of event sourcing: authoritative state tables **plus** the
append-only `decision_log` / `history`. We do **not** build pure event sourcing — it is
over-engineering here and a migration hazard.

## Import: the stage→verify→commit→log discipline (NON-NEGOTIABLE)

Import is the only part of cellarr that can destroy user data (it moves files and deletes inferior
copies). Every import follows this exact discipline. Any code path that moves, renames,
overwrites, or deletes a library file MUST follow it:

1. **Stage** — compute the full plan: source files, destination paths, which existing files would
   be replaced, hardlink-vs-copy decision, permission/ownership mapping. No filesystem mutation yet.
2. **Verify** — re-parse the actual files; confirm the match and quality; confirm destination is
   writable and has space; confirm the replaced file is really inferior per the decision engine.
3. **Commit** — perform the moves. Prefer **hardlink** within a filesystem (instant, preserves the
   seeding copy); fall back to **copy + fsync + atomic rename** across filesystems. The new file is
   fully in place and durable *before* any old file is removed.
4. **Cleanup** — only now remove replaced files; update the database in one transaction; append to
   `history` and `decision_log`.

If the process crashes at any point, the library is in a consistent state and the operation is
resumable from the log. Cross-filesystem moves never leave a partial file at the destination.
**Never delete the old file before the new file is committed.** Tests for these crash-safety
properties are mandatory — see [`specs/cellarr-fs.md`](specs/cellarr-fs.md).

## Inference on the pipeline (guardrails)

`cellarr-llm` may assist at **Parse** and **Identify** when deterministic confidence is low (see
[04-parser.md](04-parser.md)). It must **never** be the sole authority for a destructive Import
decision. A low-confidence or inference-derived match that would replace/delete a file is held for
user confirmation. Inference suggests; it does not get to overwrite your `S03E07` with a
hallucinated `S03E08`.

## Jobs, scheduling, retries

`cellarr-jobs` owns: cron-style schedules (RSS sync, metadata refresh, missing-item search,
disk-space checks), on-demand jobs (manual search/import), retry with backoff, deduplication
(don't run the same search twice concurrently), and per-resource concurrency caps. Jobs are
persisted so they survive restart. See [`specs/cellarr-jobs.md`](specs/cellarr-jobs.md).

## Failure handling

Each stage has explicit, logged failure transitions: parse-failed (→ inference fallback or
manual), identify-ambiguous (→ manual), grab-failed (→ try next release / blocklist the release),
download-failed (→ blocklist + re-search), import-failed (→ hold for review). Nothing fails
silently; every failure is in `history`/`decision_log` and surfaced in the UI.
