# Spec: cellarr-media

## Responsibility
The per-media-type modules behind the `MediaModule` trait: movie, TV (v1), music, book (post-v1).
This is what makes "one app, all media types" real — each type is a module, not a fork. Also owns
**Identify** (mapping a `ParsedRelease` to content node(s)), including anime absolute↔season/episode
mapping via scene-mapping data from `cellarr-meta`.

## Allowed dependencies
Internal: `cellarr-core`, `cellarr-meta` (via the `MetadataSource` trait — mockable). External:
`serde`, `thiserror`. No direct DB/HTTP (goes through traits).

## Public interface
- One `MediaModule` impl per media type, providing for a `ContentRef`+metadata:
  - `search_terms` — indexer queries (titles, aliases, IDs, season/ep params).
  - `match_release(parsed) -> Vec<ContentMatch>` — which content node(s), with confidence.
  - `naming_tokens` — tokens for the rename engine (`cellarr-fs`).
  - `metadata_source()` — which `MetadataSource` to refresh from.
- A registry mapping `MediaType` → `MediaModule`.

## Behavior
- The pipeline never branches on `MediaType`; it calls the module. Keep all type-specific logic here.
- Identify uses scene mappings (TheXEM + anime-lists) for anime numbering; a low-confidence or
  ambiguous match is surfaced for manual resolution, never force-fit (feeds the library-safety rule).
- Adding a media type = a new module + a `MetadataSource`, with **no changes** to parser core,
  decision engine, pipeline, download, or API.

## Test obligations
- Per-module unit tests for search_terms / match / naming against fixtures.
- Anime mapping correctness against `corpus/anime/*` (shared with parser/meta).
- A "new media type" smoke test proving the trait is sufficient (e.g. a stub module wires end to end).
- Match confidence thresholds tested at the boundary (ambiguous → manual).

## References
[02-data-model.md](../02-data-model.md), [04-parser.md](../04-parser.md),
[07-metadata-service.md](../07-metadata-service.md).
