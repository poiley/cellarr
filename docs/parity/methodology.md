# Methodology — how parity is measured

## Inputs
The title set is the curated `corpus/parse/*.toml` inputs (the `input` field of each `[[case]]`).
This is a real, provenance-tracked set of scene/p2p/anime/movie names. (Expanding this input set is
itself tracked as a gap — see [parser-gaps.md](parser-gaps.md).)

## Routing (which app is the oracle for a title)
By corpus file, since the corpus is split by concern:
- **Movie files** → Radarr `/api/v3/parse` → `.parsedMovieInfo`: `movie_title`, `movie_year`, `movie_edition`.
- **TV-episode files** → Sonarr `/api/v3/parse` → `.parsedEpisodeInfo`: `single_episode`,
  `multi_episode`, `daily_episode`, `season`, `miniseries`, `absolute_anime`.
- **Generic TV files** → Sonarr (title/group/quality only): `quality`, `language`, `release_group`,
  `unicode`, `proper_repack`.

## Field mapping (cellarr `ParsedRelease` ↔ oracle)
| Concept | cellarr | Sonarr | Radarr |
|---------|---------|--------|--------|
| title | `clean_title` | `parsedEpisodeInfo.seriesTitle` | `parsedMovieInfo.movieTitle` |
| season | `Coordinates::Episode.season` / `SeasonPack.season` | `seasonNumber` | — |
| episodes | `Coordinates::Episode.episode` (all) | `episodeNumbers[]` | — |
| absolute | `Coordinates::Absolute.number` / `Episode.absolute` | `absoluteEpisodeNumbers[]` | — |
| daily date | `Coordinates::Daily.date` | `airDate` | — |
| full season | `Coordinates::SeasonPack` present | `fullSeason` | — |
| group | `group` | `releaseGroup` | `releaseGroup` |
| quality | `resolve_quality(parsed).name` | `quality.quality.name` | `quality.quality.name` |
| year | `year` | — | `year` |
| edition | `edition` | — | `edition` |
| proper/repack | `proper_repack` | `quality.revision.version>1` / `.isRepack` | same |

## Normalization (avoid false mismatches)
- **Title:** lowercase, drop non-alphanumeric (keep spaces), collapse whitespace, trim. (So
  "The.Series" vs "The Series" match; punctuation differences are not counted as parser gaps.)
- **Group/edition:** case-insensitive, trimmed; empty string ≡ absent.
- **Quality name:** exact string compare against the app's vocabulary. A name-vocabulary
  difference *is* a real gap (cellarr should speak the same quality names) and is recorded.
- **Numbers:** set/exact compare.

## What counts as a mismatch
For each title, each *category-relevant* field is compared. A field is a **mismatch** when both
sides produced a value and they differ after normalization, **or** one produced a value the other
did not (recorded as `missing-on-cellarr` / `extra-on-cellarr`). Every mismatch is written to
`target/parity/parser-mismatches.jsonl` with `{file, input, field, cellarr, oracle}` so nothing is
lost; the catalogue in [parser-gaps.md](parser-gaps.md) is curated from that raw log.

## Parity number
Per field: `matched / compared`. Overall: the mean across compared fields, plus an
**"exact" rate** = fraction of titles where *every* category-relevant field matched. Both are
reported in [PARITY_REPORT.md](PARITY_REPORT.md) with run metadata (app versions, title count, date).

## Reproduce
```sh
just oracle        # brings up pinned Sonarr/Radarr, sets env, runs the harness
# or manually:
CELLARR_ORACLE_SONARR=http://127.0.0.1:8989 CELLARR_ORACLE_SONARR_KEY=... \
CELLARR_ORACLE_RADARR=http://127.0.0.1:7878 CELLARR_ORACLE_RADARR_KEY=... \
  mise exec -- cargo test -p cellarr-parse --test oracle -- --ignored --nocapture
```

## Caveats / threats to validity
- No populated library in the apps → we compare *parsed fields*, not series/movie *matching*
  (matching is cellarr-media's job; a separate oracle). Recorded in [decision-gaps.md](decision-gaps.md).
- Daily `seasonNumber` from Sonarr can be a sentinel; daily is compared on title + air date.
- The corpus is cellarr's own curation; titles it never imagined won't be tested until the input set grows.
