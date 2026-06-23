# Quality vocabulary parity

cellarr's quality-name set vs the originals' `GET /api/v3/qualitydefinition` (Sonarr 4.0.17 /
Radarr 6.2.1, this run). Quality *bucketing logic* is 98.3% per the parser oracle
([PARITY_REPORT.md](PARITY_REPORT.md)); this file is about the *name vocabulary* and missing buckets.

## cellarr today (core default ranking)
The default `QualityRanking` now mirrors Radarr's full worst→best catalogue (the superset that
contains every bucket), keeping the prior ordering as a subsequence so no existing relative rank
changed. Worst→best:

`Unknown, WORKPRINT, CAM, TELESYNC, TELECINE, REGIONAL, DVDSCR, SDTV, DVD, DVD-R, WEBRip-480p,
WEBDL-480p, Bluray-480p, Bluray-576p, HDTV-720p, WEBRip-720p, WEBDL-720p, Bluray-720p, HDTV-1080p,
WEBRip-1080p, WEBDL-1080p, Bluray-1080p, Bluray-1080p Remux, HDTV-2160p, WEBRip-2160p, WEBDL-2160p,
Bluray-2160p, Bluray-2160p Remux, BR-DISK, Raw-HD`.

The two remux tiers keep the **Sonarr** spelling (`Bluray-<res> Remux`) as the single canonical
internal name; the `/api/v3` shim renames them to `Remux-<res>` on the Radarr face (see G7 below).

## Sonarr vocabulary (TV)
…same HD/WEB/HDTV/SDTV/DVD set, **plus** `Bluray-576p`, **`Raw-HD`**. Uses `Bluray-1080p Remux` /
`Bluray-2160p Remux` (cellarr matches this).

## Radarr vocabulary (movies)
…HD/WEB/HDTV/SDTV/DVD set, **plus** `Bluray-576p`, `Raw-HD`, **`BR-DISK`**, **`Remux-1080p`**,
**`Remux-2160p`** (note the *different* remux naming), `DVD-R`, `DVDSCR`, `REGIONAL`, `TELECINE`,
`TELESYNC`, `WORKPRINT`. (Radarr has no `languageprofile`.)

## Gaps
1. **Missing buckets (both):** `Bluray-576p`, `Raw-HD`. — **DONE.** Added to the default ranking.
2. **Missing movie low-tiers (Radarr):** `BR-DISK` (G8), `DVD-R`, `DVDSCR`, `REGIONAL`, `TELECINE`,
   `TELESYNC`, `WORKPRINT`. — **DONE.** Added to the ranking as dedicated buckets, each backed by a
   new `Source` variant (`Workprint`/`Telesync`/`Telecine`/`Regional`/`Dvdscr`/`DvdR`/`BrDisk` and
   `RawHd`) the parser now detects, so `resolve_quality` buckets them. CAM is now CAM-only
   (`cam`/`hdcam`/`camrip`); TS/TC/WP are their own tiers. Corpus vectors added in
   `corpus/parse/quality.toml` for every new token.
3. **Remux naming divergence (G7):** cellarr/Sonarr say `Bluray-2160p Remux`; **Radarr** says
   `Remux-2160p`. The two originals genuinely **disagree**, so there is no single correct string.
   — **DONE.** One canonical internal name (the Sonarr spelling) is kept; `shim::face_quality_name`
   rewrites the two remux tiers to `Remux-<res>` on the Radarr face (and the Cellarr face, whose
   default surface is Radarr) in both `qualitydefinition` and `qualityprofile/schema`. Verified
   against both live apps' `GET /api/v3/qualitydefinition` and pinned by the
   `qualitydefinition_remux_name_differs_per_face` shim test.

## Why it matters for drop-in
Quality names are the contract between cellarr and **Recyclarr/TRaSH** (quality definitions + profile
items reference these exact names) and appear in `qualitydefinition`/`qualityprofile` responses.
Vocabulary alignment is required before Recyclarr can sync a TRaSH quality definition into cellarr.

## Action
- ~~Add `Bluray-576p`, `Raw-HD`, and the Radarr pre-retail movie tiers to `cellarr-core`'s ranking
  (+ parser source tokens for the new ones).~~ **DONE.**
- ~~Implement per-app remux name mapping in the `/api/v3` shim.~~ **DONE** (`face_quality_name`).
- Remaining: run the CF-score oracle ([decision-gaps.md](decision-gaps.md)) on a real TRaSH set with
  the widened vocabulary.

## Verification (this run)
Captured live from `GET /api/v3/qualitydefinition` (Sonarr 4.0.17 / Radarr 6.2.x, fresh containers):
Sonarr uses `Bluray-1080p Remux` / `Bluray-2160p Remux` and has `Bluray-576p` + `Raw-HD` (no movie
pre-retail tiers). Radarr uses `Remux-1080p` / `Remux-2160p` and adds `Bluray-576p`, `Raw-HD`,
`BR-DISK`, `DVD-R`, `DVDSCR`, `REGIONAL`, `TELECINE`, `TELESYNC`, `WORKPRINT`. The ranking's worst→best
order follows Radarr's `weight` column. Differential parser oracle after the change: quality field
parity 97.7% (128/131), unchanged from baseline.
