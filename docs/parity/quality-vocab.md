# Quality vocabulary parity

cellarr's quality-name set vs the originals' `GET /api/v3/qualitydefinition` (Sonarr 4.0.17 /
Radarr 6.2.1, this run). Quality *bucketing logic* is 98.3% per the parser oracle
([PARITY_REPORT.md](PARITY_REPORT.md)); this file is about the *name vocabulary* and missing buckets.

## cellarr today (core default ranking)
Bluray-480p/720p/1080p/2160p, Bluray-1080p Remux, Bluray-2160p Remux, WEBDL-480p/720p/1080p/2160p,
WEBRip-480p/720p/1080p/2160p, HDTV-720p/1080p/2160p, SDTV, DVD, CAM, Unknown.

## Sonarr vocabulary (TV)
…same HD/WEB/HDTV/SDTV/DVD set, **plus** `Bluray-576p`, **`Raw-HD`**. Uses `Bluray-1080p Remux` /
`Bluray-2160p Remux` (cellarr matches this).

## Radarr vocabulary (movies)
…HD/WEB/HDTV/SDTV/DVD set, **plus** `Bluray-576p`, `Raw-HD`, **`BR-DISK`**, **`Remux-1080p`**,
**`Remux-2160p`** (note the *different* remux naming), `DVD-R`, `DVDSCR`, `REGIONAL`, `TELECINE`,
`TELESYNC`, `WORKPRINT`. (Radarr has no `languageprofile`.)

## Gaps
1. **Missing buckets (both):** `Bluray-576p`, `Raw-HD`. — add to the ranking.
2. **Missing movie low-tiers (Radarr):** `BR-DISK` (G8), `DVD-R`, `DVDSCR`, `REGIONAL`, `TELECINE`,
   `TELESYNC`, `WORKPRINT`. cellarr has `CAM` but not the rest of the pre-retail tier. — add for movie
   libraries; needs parser detection of these source tokens too.
3. **Remux naming divergence (G7):** cellarr/Sonarr say `Bluray-2160p Remux`; **Radarr** says
   `Remux-2160p`. The two originals genuinely **disagree**, so there is no single correct string.
   Resolution: keep one canonical internal name and **map per emulated app in the `/api/v3` shim**
   (present Sonarr-style names on a TV library, Radarr-style on a movie library) rather than changing
   the core bucket.

## Why it matters for drop-in
Quality names are the contract between cellarr and **Recyclarr/TRaSH** (quality definitions + profile
items reference these exact names) and appear in `qualitydefinition`/`qualityprofile` responses.
Vocabulary alignment is required before Recyclarr can sync a TRaSH quality definition into cellarr.

## Action
- Add `Bluray-576p`, `Raw-HD`, and the Radarr pre-retail movie tiers to `cellarr-core`'s ranking
  (+ parser source tokens for the new ones).
- Implement per-app remux name mapping in the `/api/v3` shim (ties into the `qualitydefinition`
  endpoint, which is currently unimplemented — see [api-v3-gaps.md](api-v3-gaps.md)).
- Then run the CF-score oracle ([decision-gaps.md](decision-gaps.md)) on a real TRaSH set.
