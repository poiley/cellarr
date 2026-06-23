# 13 — Upstream repositories: what to learn, what to reuse, licensing

This is the single source of truth for **where the domain knowledge lives** and **how we may use
it**. It consolidates research done against the live upstream repos (June 2026). Where a fact is
uncertain it says so — verify before relying on it.

> **Golden rule:** learn *behavior* and reuse *facts/data*; do not transcribe *code*. The
> clean-room rules and the license decision are in
> [agents/legal-and-licensing.md](agents/legal-and-licensing.md). All of Sonarr/Radarr/Lidarr/
> Readarr/Prowlarr are **GPLv3**. SRCL is **MIT**.

## The *arr family (the domain knowledge)

All share a common **.NET / C#** base descended from **NzbDrone** (`src/NzbDrone.Core`,
`NzbDrone.Common`). Radarr is a Sonarr fork; Lidarr/Readarr/Prowlarr share the lineage; unified
under the **Servarr** org.

### Sonarr — TV parsing & episode logic
- Repo: `Sonarr/Sonarr` (.NET 6). Parser: **`src/NzbDrone.Core/Parser/`** —
  `Parser.cs` (main entry: `ParseTitle`, `ParseSeriesName`, `ParseReleaseGroup`,
  `CleanSeriesTitle`; ~88 title regexes), `QualityParser.cs`, `LanguageParser.cs`,
  `ParsingService.cs`, `SceneChecker.cs`. **Note:** release-group parsing lives *inside* `Parser.cs`
  (Sonarr has no separate `ReleaseGroupParser.cs`).
- **Test fixtures (the corpus source):** `src/NzbDrone.Core.Test/ParserTests/` — ~25 files, NUnit
  `[TestCase]` rows asserted with FluentAssertions, split by type:
  `SingleEpisodeParserFixture.cs`, `MultiEpisodeParserFixture.cs`, `DailyEpisodeParserFixture.cs`,
  `MiniSeriesEpisodeParserFixture.cs`, `SeasonParserFixture.cs`,
  `AbsoluteEpisodeNumberParserFixture.cs` (anime), `QualityParserFixture.cs`,
  `LanguageParserFixture.cs`, `ReleaseGroupParserFixture.cs`, `UnicodeReleaseParserFixture.cs`.
  Estimated **~1,500–2,000 `[TestCase]` rows**. These input→expected vectors are what we extract
  into `corpus/parse/`.
- Anime numbering: `src/NzbDrone.Core/DataAugmentation/Xem/` (`XemProxy.cs`, `XemService.cs`) maps
  scene↔TVDB via **TheXEM** (`thexem.info`); `DataAugmentation/Scene/` combines it with Sonarr's own
  list. Status: thexem.info appears operational/maintained (don't confuse with the dormant
  self-host server repo `NMe84/xem`).
- Metadata proxy: **`skyhook.sonarr.tv`** proxies **TheTVDB** (server not open-sourced).

### Radarr — movie parsing
- Repo: `Radarr/Radarr` (.NET 8). Same `src/NzbDrone.Core/Parser/` layout, **plus** a real
  `ReleaseGroupParser.cs` and a `RomanNumerals/` dir. Movie-specific: `ParseMovieTitle`,
  `ParseMoviePath`, `ParseEdition`, `ParseImdbId`, `ParseTmdbId`, `ParseHardcodeSubs`;
  `ReportMovieTitleRegex`; inline `year` capture with negative lookahead to avoid matching `1080p`;
  dedicated `EditionRegex` (Director's Cut, Extended, IMAX, Remastered…).
- Test fixtures: `src/NzbDrone.Core.Test/ParserTests/` incl. `EditionParserFixture.cs`,
  `SlugParserFixture.cs`, `RomanNumeralTests/`. → `corpus/parse/movie_*.toml`.
- Metadata proxy: **`api.radarr.video`** proxies **TMDb**; backend repo `Radarr/RadarrAPI.TMDB`.

### Prowlarr — indexers
- Repo: `Prowlarr/Prowlarr` (.NET). Centralizes indexers; pushes Torznab/Newznab definitions into
  apps via sync (Disabled / Add-and-Remove-Only / Full Sync). Exposes a per-indexer proxy endpoint
  `{prowlarr}/{indexer_id}/api?apikey=...`.
- **Indexer definitions (Cardigann YAML):** in a **separate repo** `Prowlarr/Indexers` at
  `definitions/v11/*.yml` (~552 files; "500+"). Lineage: cardigann (Go) → Jackett (C#) → Prowlarr.
  Definitions are **data, not code**. **License: the `Prowlarr/Indexers` repo has no declared
  license (`license: null`).** → We build our own engine; we do **not** vendor these YAML files.

### Lidarr / Readarr
- Same family, GPLv3. Music (Lidarr → MusicBrainz) and books (Readarr → Goodreads/OpenLibrary
  lineage). Relevant when we add those media modules. (Note: Readarr's upstream is effectively
  unmaintained as of 2025–26 — treat its behavior as reference, not gospel.)

## Community data sources (reuse-friendly)

- **TRaSH-Guides** (`trash-guides.info`) — custom-format regexes + recommended scores as JSON
  (`trash_scores` keyed by `default`/`anime`/`german`…), applied via Recyclarr/Configarr. cellarr
  imports these directly. See [05-decision-engine.md](05-decision-engine.md).
- **TheXEM** (`thexem.info`) — scene↔TVDB episode mapping (anime/scene). Live API.
- **Anime-Lists** (`Anime-Lists/anime-lists`, `anime-list-master.xml`) — AniDB↔TheTVDB mapping;
  actively maintained; used by Plex/Jellyfin/Kodi.

## Metadata sources

See [07-metadata-service.md](07-metadata-service.md) for the full table (auth, rate limits,
self-host, licensing). Summary: **TMDb** (movies; free key, no-commercial-use clause), **TheTVDB
v4** (TV; paid, the one hard external dep), **MusicBrainz** (music; CC0, full dumps, 1 req/s/IP),
**OpenLibrary** (books; CC0, full dumps), **AniDB** (anime; client registration, aggressive bans,
no bulk).

## UI

- **SRCL** (`internet-development/www-sacred`, npm `srcl`) — **MIT**. Next.js 16 / React 19.
  Canonical catalog: `reference/www-sacred/components/AGENTS.md`; raw sources at
  `https://sacred.computer/llm/components/<Name>.tsx.txt`. See [10-ui.md](10-ui.md).

## Rust crate choices (decided once, here)

| Need | Crate | Why / notes |
|------|-------|-------------|
| async runtime | **tokio** | the ecosystem standard; everything builds on it |
| HTTP server | **axum** | tokio-team, Tower middleware, clean extractors |
| HTTP client | **reqwest** | de-facto async client (hyper/tokio) |
| SQL (SQLite **+** Postgres, compile-time checked) | **sqlx** | `query!` macros verify SQL+types vs both; offline `.sqlx` for CI. (SQLite checks shallower than PG.) |
| full-text search | **SQLite FTS5** (built-in); **tantivy** if outgrown | no separate service |
| regex | **regex** (+ **fancy-regex** where lookaround needed) | std `regex` has **no lookaround/backrefs** by design → linear time, no catastrophic backtracking. See [04-parser.md](04-parser.md). |
| in-process cache | **moka** | concurrent TTL/size cache; no Redis |
| rate limiting | **governor** | per-host GCRA limiter for indexers/metadata |
| parallel CPU | **rayon** | data-parallel parse/hash off the reactor |
| WASM plugins | **wasmtime** (v46+); **Extism** optional | mature Component Model / WIT in 2026 |
| background jobs | **apalis** (note: pre-1.0 RC) or **tokio-cron-scheduler** | persisted jobs/cron/retry |
| observability | **tracing** (+ `tracing-subscriber`, optional `tracing-opentelemetry`) | structured async tracing |
| serialization | **serde** (+ serde_json) | universal |
| config | **figment** (or `config`) | layered defaults→file→env |
| property tests | **proptest** | parser/scoring invariants |
| test fixtures/tables | **rstest** | `#[case(...)]` maps cleanly onto corpus vectors |

## Uncertainties flagged by research (verify before relying)

- TheTVDB exact license tiers and (unpublished) rate limits.
- TMDb ~40–50 req/s is community guidance, not contractual; commercial use needs an agreement.
- qBittorrent 5.1 "Host-header validation default-on" unconfirmed; the `/login` success-check
  regression in late-2025 dev builds is confirmed (broke the originals' check; client-side fix).
- `apalis` is a release candidate, not GA — re-evaluate vs a simpler scheduler at scaffold time.
- `Prowlarr/Indexers` has **no license** — do not vendor those YAMLs; revisit if our license/model
  needs to depend on them.
