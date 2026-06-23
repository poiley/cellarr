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
  `OpenLibrarySource`, `AniDbSource` post-v1.
- Trait methods: `search`, `fetch` (with child structure), `updates(since)`, `images`,
  `scene_mapping` (TheXEM + anime-lists).
- A normalized schema returned to `cellarr-media` regardless of source.
- A cache layer with per-source TTLs (in-process `moka` + persisted cache table via `cellarr-db`).

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
- Scene-mapping correctness against `corpus/anime/*` (shared with parser/media).
- Cache behavior: TTL/expiry/stampede protection.
- Standalone vs embedded both exercised.
- Opt-in live drift suite detects upstream shape changes.

## References
[07-metadata-service.md](../07-metadata-service.md), [13-upstream-repos.md](../13-upstream-repos.md).
