# Spec: cellarr-core

## Responsibility
The shared heart: domain types, the cross-crate **traits** (the seams), and the **pipeline state
machine**. It is the vocabulary every other crate speaks. It contains **no I/O**, no database, no
HTTP — pure types and logic, so it compiles fast and is trivially testable. It is the one crate with
**no internal dependencies**.

## Allowed dependencies
Internal: none. External: `serde`, `thiserror`, time/uuid utilities, and pure-logic helpers only.
No `tokio`-required APIs in the core types (keep them runtime-agnostic where practical); no `sqlx`,
no `reqwest`.

## Public interface
- **Domain types:** `MediaType`, `Coordinates` (the numbering enum: `Movie` / `Episode` / `Daily` /
  `SeasonPack` / `Absolute` / `Track` / `Book` — the parser emits the transient TV variants, Identify
  remaps them), `ContentRef`, `ContentNode` (+ `ContentKind`), `MediaFile`, `ParsedRelease` (+
  per-field confidence), `Release` (a candidate from an indexer), `ContentMatch`, `QualityProfile`,
  `QualityDefinition`, `Quality` (+ `QualityRanking` and the `resolve_quality` resolver),
  `CustomFormat`, `Decision`/`Verdict`, `GrabRequest`, `Grab` (+ `GrabStatus`), `ImportPlan`
  (+ `PlannedMove`, which carries `replaced_path` for replacements at a distinct path), config types
  (`RootFolder`, `IndexerConfig`, `DownloadClientConfig`, `NotificationConfig`), history/decision-log
  records.
- **Seam traits** (definitions live here; impls live in their crates):
  - `MediaModule` — search terms / match / naming / metadata source (see [02-data-model.md](../02-data-model.md)).
  - `MetadataSource` — search / fetch / updates / images / scene_mapping (see [07](../07-metadata-service.md)).
  - `Indexer`, `DownloadClient` — integration seams (see [06](../06-integrations.md)).
  - repository traits (consumed by all; implemented by `cellarr-db`): `ContentRepository`
    (`get` / `monitored_missing` / `upsert` / `children`), `MediaFileRepository`
    (`create` / `get` / `list_for_content` / `delete`), `GrabRepository`
    (`create` / `get` / `set_download_id` / `set_status`), `HistoryRepository`,
    `DecisionLogRepository`, `ProfileRepository`.
- **Pipeline state machine:** the `Stage` enum, transition types, and the pure transition logic for
  Discover→Parse→Identify→Decide→Grab→Track→Import→Rename→Notify ([03-pipeline.md](../03-pipeline.md)).
  Execution/scheduling lives in `cellarr-jobs`; *the rules* live here.

## Behavior
- Types must express all four media types without per-type branching outside `Coordinates`/`MediaModule`.
- The pipeline logic is pure and deterministic given its inputs; side effects belong to other crates.
- Every state transition produces a decision-log/history record value (persisted elsewhere).

## Test obligations
- Unit tests for type invariants (e.g. `Coordinates` round-trips to/from its tagged JSON form).
- State-machine tests: every legal transition and every failure transition is exercised; illegal
  transitions are unrepresentable or rejected.
- Doc tests on public types.

## References
[01-architecture.md](../01-architecture.md), [02-data-model.md](../02-data-model.md),
[03-pipeline.md](../03-pipeline.md), [15-glossary.md](../15-glossary.md).
