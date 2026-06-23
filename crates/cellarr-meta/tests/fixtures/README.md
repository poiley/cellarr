# Record/replay fixtures — cellarr-meta

**These are SYNTHETIC fixtures**, hand-authored to mirror the *documented* response
shapes of each source. They are **not** captured from a live API (no live source
touches the CI path — `docs/07-metadata-service.md`). The field names and nesting
match each provider's public docs so the normalizers exercise the real decode
path; the values (titles, ids, dates) are invented and trimmed to the fields the
normalizer reads.

| File | Source / shape | Used to assert |
|------|----------------|----------------|
| `tmdb_search_movie.json` | TMDb `GET /3/search/movie` | search → `SearchResult` normalization (movies) |
| `tmdb_movie.json` | TMDb `GET /3/movie/{id}?append_to_response=images` | fetch → `Metadata` (title/year/imdb id/images) |
| `tvdb_search_series.json` | TheTVDB v4 `GET /v4/search?type=series` | search → `SearchResult` normalization (TV) |
| `tvdb_series_extended.json` | TheTVDB v4 `GET /v4/series/{id}/extended` | fetch → `Metadata` with season/episode child structure + absolute numbers |
| `anime_list_entry.xml` | `Anime-Lists/anime-lists` `<anime>` element | anime-lists → `SceneMap`; absolute→episode remap across seasons |
| `xem_map_all.json` | TheXEM `map/all` response | TheXEM → `SceneMap`; per-episode rows collapsed to runs |

If a source changes its shape, the opt-in live drift suite (future) is what
catches it; these fixtures pin the *normalization contract*, not upstream truth.
