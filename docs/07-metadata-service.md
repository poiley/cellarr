# 07 — The metadata service (Skyhook rebuild)

The originals do **not** call TheTVDB/TMDb directly from your box — they go through a metadata
**proxy** (`skyhook.sonarr.tv` for Sonarr/TheTVDB; `api.radarr.video` for Radarr/TMDb) that holds
the API key, caches aggressively, and normalizes the data. cellarr rebuilds this as
**`cellarr-meta`**, which can run **embedded** in the daemon or as a **standalone, self-hostable
service** (`meta-service/`).

## Why a proxy/normalizer at all

- **Key custody & rate limits:** one place that holds keys and enforces source rate limits.
- **Caching:** metadata changes slowly; long-TTL caching cuts source load and latency dramatically.
- **Normalization:** the rest of cellarr consumes one clean schema regardless of source quirks.
- **Self-host story:** privacy-minded users can run their own instance; casual users can point at a
  shared default instance and need no keys. Both must work.

> We must **never** proxy through the originals' Skyhook/RadarrAPI — that violates their ToS and
> they will block us. We rebuild the role with our own keys/infra. The point of this doc is to make
> that a first-class, self-hostable component.

## Sources, auth, and self-host reality

This table drives the design. The blunt truth: only music and books can run fully offline from
dumps; TV, movies, and anime require live API keys + caching.

| Source | Used for | Auth | Rate limit (respect these) | Self-host offline? |
|--------|----------|------|----------------------------|--------------------|
| **TMDb** | movies (+ TV imagery) | free API key/bearer | soft ~40–50 req/s/IP; honor HTTP 429 | **Partial** — daily ID exports seed a crawl; fetch by ID live. *Commercial use needs a separate agreement.* |
| **TheTVDB v4** | TV | licensed key, or user key + subscriber PIN | unpublished — be conservative | **No** — no dumps; live API + `/updates`. *Paid; the one hard external dependency.* |
| **MusicBrainz** | music | anonymous reads; descriptive `User-Agent` **mandatory** | **~1 req/s per IP** (auth does not raise it) | **Yes, fully** — twice-weekly Postgres dumps + replication (`musicbrainz-docker`). Core data CC0. |
| **OpenLibrary** | books | open; `User-Agent` w/ email recommended | ~1 req/s anon | **Yes, fully** — monthly dumps; data CC0. |
| **AniDB** | anime identity | **mandatory client registration** (`client`/`clientver`) | HTTP 1 req/2s; UDP 1/4s; **aggressive bans** | **No** — only the daily anime-titles dump for matching; metadata is live-only. |

Plus mapping data (not "metadata" but identity reconciliation):
- **TheXEM** (`thexem.info`) — scene ↔ TVDB episode number mapping for anime/scene releases.
- **anime-lists** (`Anime-Lists/anime-lists`) — AniDB ↔ TheTVDB mapping; actively maintained.

## Design

`cellarr-meta` exposes a normalized interface to the rest of cellarr via the **`MetadataSource`**
trait (defined in `cellarr-core`, consumed by `cellarr-media`):

- `search(query) -> candidates` — find a title by name/year/IDs.
- `fetch(id) -> normalized metadata` — full record (with child structure: seasons/episodes,
  albums/tracks, etc.).
- `updates(since) -> changed ids` — for incremental refresh.
- `images(id) -> artwork refs`.
- `scene_mapping(id) -> Coordinates remap` — for anime/scene numbering (TheXEM + anime-lists).

Behind the trait, each source is an adapter that handles its own auth, rate limiting (`governor`,
per-source, conservative — AniDB and MusicBrainz especially), retry/backoff, and quirk-shielding.
A **cache layer** (`moka` in-process + a persisted cache table) sits in front with per-source TTLs.

### Bring-your-own-key + shared default
- Users may configure their own TMDb/TheTVDB keys (and must, for a fully private setup).
- A shared default instance can be offered so casual users need no keys — but the daemon must run
  and function (degrade gracefully) if no metadata source is reachable.

### Offline / self-host modes
- **MusicBrainz / OpenLibrary**: support pointing at a locally-hosted dump-backed instance.
- **TMDb / TheTVDB / AniDB**: live-only; cache hard. Document the key/subscription requirements
  plainly so users aren't surprised.

## Testing

- **Record/replay** every source: recorded API responses → asserted normalization into the common
  schema. No live source on the CI critical path.
- **Mapping correctness**: a corpus of anime titles with known absolute↔season/episode expectations
  (shared with the parser's `absolute_anime.toml`) validates the scene-mapping path end to end.
- **Cache behavior**: TTL/expiry/stampede-protection tests.
- An opt-in live smoke suite verifies real sources still match recorded shapes (drift detection).

See [`specs/cellarr-meta.md`](specs/cellarr-meta.md).

## Uncertainties to verify before relying on them

- TheTVDB's exact license dollar-tiers and rate limits (unpublished).
- TMDb's ~40–50 req/s figure is community guidance, not contractual.
- TMDb's no-commercial-use clause and TheTVDB's paid licensing are the two licensing landmines for
  any *monetized* distribution — flag to a human before any commercial use.
