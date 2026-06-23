# Spec: cellarr-meta

## Responsibility
The metadata service (Skyhook rebuild): normalize + cache identity/descriptive data from external
sources, and provide scene-mapping data for anime numbering. Runs **embedded** in the daemon or as a
**standalone, self-hostable** binary (`meta-service/`).

## Allowed dependencies
Internal: `cellarr-core`. External: `reqwest`, `tokio`, `moka` (cache), `governor` (per-source rate
limits), `serde`, an XML parser (anime-lists/AniDB), `thiserror`. May expose an axum endpoint when
run standalone.

## Public interface
- `MetadataSource` impls: `TmdbSource` (movies), `TheTvdbSource` (TV) for v1; `MusicBrainzSource`,
  `OpenLibrarySource`, `AniDbSource` post-v1. Each is generic over an injected `http::Fetcher` so the
  same normalization logic runs against live (`ReqwestFetcher`) and recorded (`RecordedFetcher`)
  bytes.
- **Trait methods** (the frozen `cellarr_core::MetadataSource` seam): `search`, `fetch` (with child
  structure), `scene_mapping` (TheXEM + anime-lists). The core trait does **not** carry `updates` or
  `images`: `images` is exposed as an inherent method on each source (derived from the `fetch`
  payload, since artwork is fetched alongside the record); `updates(since)` is deferred until the
  persisted cache (and its change-tracking) lands — there is no incremental-refresh consumer yet, so
  adding it to the seam now would be speculative. Both are noted here so the doc stays the plan of
  record.
- A normalized schema (`Metadata`/`SearchResult`/`ChildNode`/`Image`) returned to `cellarr-media`
  regardless of source; consumers wanting raw payloads use the trait's `serde_json::Value` form.
- A cache layer with per-source TTLs: **in-process `moka` now** (stampede-protected via coalescing
  loads; failed loads are not cached). The **persisted cache table via `cellarr-db` is a deliberate
  follow-up** — `cellarr-meta` does not depend on `cellarr-db`.

## Behavior
- **Per-source rate limiting**, conservative: MusicBrainz ~1 req/s/IP with a descriptive
  `User-Agent`; AniDB strict (client registration, HTTP 1/2s, aggressive bans) — be defensive.
- **Bring-your-own-key** (TMDb/TheTVDB) and an optional shared default instance; the daemon must run
  and degrade gracefully if no source is reachable (offline non-negotiable).
- Support locally-hosted dump-backed instances for MusicBrainz/OpenLibrary; TMDb/TheTVDB/AniDB are
  live-only → cache hard.
- **Never** proxy through the originals' Skyhook/RadarrAPI (ToS).
- TheTVDB paid/keys and TMDb no-commercial-use are licensing landmines — surfaced, not hidden.

## Test obligations
- **Record/replay** per source → asserted normalization into the common schema. No live source in CI.
  Implemented via the `http::Fetcher` seam: `RecordedFetcher` serves the synthetic fixtures in
  `tests/fixtures/` (documented TMDb/TheTVDB/TheXEM/anime-lists shapes, labelled synthetic). Six
  fixtures back ten record/replay assertions (search + fetch-with-children + images + no-key
  degradation + a 429 → typed `Http` error).
- Scene-mapping correctness against `corpus/anime/*` (shared with parser/media): every
  `absolute_to_season_episode.toml` case is reproduced by `SceneMap::remap_absolute`, and every
  `unmapped_absolute.toml` case is **surfaced** as `MetaError::Unmappable` (distinguishing `unmapped`
  from `malformed`) — never force-fit, per the library-safety rule.
- Cache behavior: TTL/expiry/stampede protection (coalesced loads; failed loads are not cached).
- Standalone vs embedded both exercised (the `standalone` feature builds + tests; the in-process
  adapters are identical in both modes).
- Opt-in live drift suite detects upstream shape changes. **Deferred** — there is no live-source job
  yet; the synthetic fixtures pin the normalization contract until one is added.

## References
[07-metadata-service.md](../07-metadata-service.md), [13-upstream-repos.md](../13-upstream-repos.md).
