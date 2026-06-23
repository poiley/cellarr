# Oracle setup

How the differential oracle is stood up locally. Local-only (no publishing); the apps run in
Docker via OrbStack.

## Images & versions (this run)

| App | Image | App version | Image age at pull |
|-----|-------|-------------|-------------------|
| Sonarr | `lscr.io/linuxserver/sonarr:latest` | **4.0.17.2952** | 10 days |
| Radarr | `lscr.io/linuxserver/radarr:latest` | **6.2.1.10461** | 38 hours |

> The compose file (`tests/oracle/docker-compose.yml`) had placeholder pins (`sonarr:4.0.10`,
> `radarr:5.14.0`). We pulled `:latest` and recorded the real versions above. **TODO:** pin the
> compose to these exact versions/digests for reproducibility once the harness is stable.

## Bring-up (manual, what the harness automates)

```sh
docker run -d --name oracle-sonarr -p 127.0.0.1:8989:8989 --tmpfs /config lscr.io/linuxserver/sonarr:latest
docker run -d --name oracle-radarr -p 127.0.0.1:7878:7878 --tmpfs /config lscr.io/linuxserver/radarr:latest
```

- `--tmpfs /config` → ephemeral config, fresh each boot (no state to clean up).
- First boot writes `/config/config.xml` within ~2s; the web API is up shortly after.
- **API key** is auto-generated in `config.xml`:
  `docker exec oracle-sonarr sed -n 's:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p' /config/config.xml`
- Auth: send `X-Api-Key: <key>` on every request. The API key works for `/api/v3/*` regardless of
  the forms-auth UI setting, so no login flow is needed.

## The parse endpoint (parser oracle surface)

`GET /api/v3/parse?title=<release string>` — no setup/library needed; pure parse.

**Sonarr** response (relevant fields), under `.parsedEpisodeInfo`:
- `seriesTitle`, `seasonNumber`, `episodeNumbers[]`, `absoluteEpisodeNumbers[]`, `releaseGroup`,
  `quality.quality.name`, `quality.revision` (proper/repack), `languages[].name`, `releaseTitle`,
  plus `fullSeason`, `special`, `isDaily`, `airDate`, `releaseTokens`.
- Top-level also: `customFormatScore`, `customFormats[]`, `episodes[]` (matched, empty without a library), `languages[]`.

**Radarr** response, under `.parsedMovieInfo`:
- `movieTitle`, `year`, `edition`, `quality.quality.name`, `releaseGroup`, `languages[].name`,
  `imdbId`, `tmdbId`, `originalTitle`, `releaseTitle`, `releaseHash`.
- Top-level also: `customFormatScore`, `customFormats[]`, `languages[]`.

Verified samples:
- Sonarr `The.Series.S02E15.1080p.BluRay.x264-GROUP` → series "The Series", S2, ep [15],
  quality "Bluray-1080p", group "GROUP". ✓
- Radarr `The.Matrix.1999.1080p.BluRay.x264-AMIABLE` → "The Matrix", 1999, quality "Bluray-1080p",
  group "AMIABLE". ✓

## Obstacles & notes
- `docker images <a> <b>` (multi-arg) errors — query one repo at a time.
- The apps return matched `episodes`/`movie` only with a populated library; for **parser** parity we
  only compare the *parsed fields*, not matches (matching is cellarr-media's job, oracle'd separately).
- `quality.quality.name` is the app's quality *name* (e.g. "Bluray-1080p"); cellarr's `Quality.name`
  must be mapped to the same vocabulary — see [methodology.md](methodology.md).

## Teardown
```sh
docker rm -f oracle-sonarr oracle-radarr
```
