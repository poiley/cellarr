# Parser gaps — the catalogue

Every parser difference found by the oracle, classified. Source data:
`target/parity/parser-mismatches.jsonl` (raw, regenerated each run). Numbers in
[PARITY_REPORT.md](PARITY_REPORT.md).

Each entry: **what**, an example, **classification**, and **status**.

Classification:
- **REAL** — cellarr is wrong / weaker than the originals; should be fixed.
- **ARTIFACT** — a representation/routing/oracle quirk, not a cellarr bug (cellarr is correct or
  even stronger). Tracked so we don't "fix" the wrong thing, and to refine the harness.

---

## REAL gaps (actionable)

### G1 — Quality: resolution-only releases resolve to `unknown` instead of `HDTV-<res>` ⬅ biggest
When a release has a resolution but **no explicit source** (common in anime and raw web), Sonarr
defaults the source to HDTV: `[1080p]` → `HDTV-1080p`, `(720p)` → `HDTV-720p`. cellarr's
`resolve_quality` requires a source and returns `unknown`.
- `[SubsPlease] Show Title - 1071 (1080p) [ABCD1234].mkv` → cellarr `unknown`, oracle `hdtv-1080p`
- `[HorribleSubs] Another Anime - 12 [720p].mkv` → cellarr `unknown`, oracle `hdtv-720p`
- ~6 cases. **Status: FIXED** (resolution-only → HDTV-<res>) — see PARITY_REPORT run 2.

### G2 — Quality: source-only `HDTV` with no resolution resolves to `unknown` instead of `SDTV`
`...HDTV.x264` with no resolution token → Sonarr `SDTV`. cellarr returns `unknown`.
- `Conan.2020.01.07.HDTV.x264-SORNY` → cellarr `unknown`, oracle `sdtv`
- `The.Office.US.S05E14.HDTV.XviD-XOR` → cellarr `unknown`, oracle `sdtv`
- `Jimmy.Kimmel.2018.12.25.480p.HDTV.x264-aAF` → cellarr `unknown`, oracle `sdtv` (480p+HDTV = SD)
- 3 cases. **Status: FIXED** (HDTV w/o ≥720p → SDTV) — see run 2.

### G3 — Miniseries "Part N" not recognized
`Generation.Kill.Part.4...` → Sonarr: title "Generation Kill", season 1, episode [4]. cellarr
leaves "Part 4" in the title, no season/episode. One title, three fields (title/season/episodes).
- **Status: DEFERRED (real).** Fix = a `Part N` numbering rule → `Episode{season:1, episode:N}` and
  strip "Part N" from the title. **Risk:** "Part N" also appears in legitimate *movie* titles
  ("...Part 1"), so the rule needs guarding (TV context only, no movie-year-as-sole-signal). cellarr's
  own corpus currently encodes the *opposite* choice (`miniseries.toml`: "Part N ... no S/E surfaced"),
  so closing this means a deliberate corpus + parser change. Not rushed in this pass.

### G4 — Series title drops a disambiguating year
`Watchmen.2019.S01E09...` → Sonarr keeps series title "Watchmen 2019"; cellarr strips to "Watchmen".
The year after a series name is part of the title (disambiguates remakes). cellarr removes it.
- **Status: DEFERRED (real).** Fix = retain a year that sits directly between the series name and the
  `SxxEyy` marker (it's part of the title), while still stripping a standalone release year. **Risk:**
  the title cleaner strips years globally; a targeted change could regress other titles, so it needs
  its own corpus cases before landing. Not rushed in this pass.

### G7 — Remux quality name differs from Radarr's vocabulary
`...2160p.BluRay.REMUX...` → cellarr `Bluray-2160p Remux`, Radarr `Remux-2160p`. cellarr detects the
remux correctly; only the *name* differs. **Note:** Sonarr and Radarr **disagree** here historically
(Sonarr "Bluray-2160p Remux", Radarr newer "Remux-2160p"), so there is no single oracle answer.
- **Status: DEFERRED (vocabulary).** Part of the quality-vocabulary alignment follow-up; pick a
  canonical set and a per-app mapping in the `/api/v3` shim rather than changing the core bucket name.

### G8 — No full-disc (BR-DISK) quality bucket
`Movie.2019.4K.UHD.BluRay.2160p.HDR.HEVC-GRP` (no remux, no encode marker) → Radarr `BR-DISK` (raw
disc); cellarr → `Bluray-2160p`. cellarr has no full-disc bucket, so it collapses discs into the
encoded Bluray tier.
- **Status: DEFERRED (real, niche).** Fix = add a disc/`BR-DISK` (and `Raw-HD`) quality + detection
  of full-disc structure (`UHD BluRay` / `BDMV` / `ISO` without an encode). Low frequency; tracked.

### G5 — Edition: only the first keyword captured; some editions unrecognized
cellarr captures a single token where Radarr captures the full edition phrase, and misses "Final Cut".
- `Blade.Runner.1982.The.Final.Cut...` → cellarr `∅`, oracle `final cut` (unrecognized)
- `Lord.of.the.Rings...Extended.Edition...` → cellarr `extended`, oracle `extended edition`
- `Avatar...Extended.Collectors.Edition...` → cellarr `extended`, oracle `extended collectors edition`
- `The.Thing...Theatrical.Cut...` → cellarr `theatrical`, oracle `theatrical cut`
- `Watchmen...Directors.Cut...` → cellarr `director's cut`, oracle `directors cut` (apostrophe)
- **Status: "Final Cut" FIXED; the rest is an intentional DIVERGENCE.** Adding "Final Cut" closed a
  true miss. The remaining differences are cellarr's deliberate **canonicalization**: it normalizes
  editions to a stable spelling ("Extended", "Director's Cut", "Theatrical") for consistent
  custom-format matching, whereas Radarr preserves the raw phrase ("extended edition", "directors
  cut", "theatrical cut"). Radarr is itself inconsistent (returns ∅ for `Redux`/`Criterion` that
  cellarr captures — see A3). We keep canonicalization and **allow-list** these as expected
  divergences; aligning the exact strings to a chosen vocabulary is a separate follow-up.

### G6 — Hyphenated release group truncated
`Movie.2019.1080p.BluRay.x264-D-Z0N3` → Sonarr group "d-z0n3"; cellarr captures only "z0n3" (stops
at the last hyphen). Release groups can contain hyphens.
- **Status: FIXED** (group capture allows internal hyphens) — see run 2.

---

## ARTIFACTS (not cellarr bugs — recorded, harness refined)

### A1 — daily/anime `season = 0` sentinel
Sonarr assigns `seasonNumber = 0` to daily shows and absolute-numbered anime (no real season).
cellarr correctly uses `Coordinates::Daily{date}` / `Coordinates::Absolute{number}` and leaves
season unset. 12 "season" mismatches are this sentinel. **Harness refinement:** do not compare
`season` for the `daily_episode`/`absolute_anime` categories (compare daily air-date and absolute
number instead). cellarr's daily-date and absolute extraction already match (see field rates).

### A2 — CAM/HDCAM routed to Sonarr
`quality.toml` contains movie-vocabulary titles (CAM, HDCAM). The harness routes that file to
Sonarr, which has no CAM quality and maps it to SDTV. cellarr correctly reports `cam`. **Harness
refinement:** route CAM/HDCAM (movie qualities) to Radarr, or split the generic quality corpus by
domain.

### A3 — Originals return ∅ where cellarr parses successfully (cellarr stronger)
- `[Group] Title Of Anime - 5 [480p].mkv` → Sonarr returns empty seriesTitle/group/absolute;
  cellarr parses title "Title Of Anime", group, absolute [5]. (Sonarr needs the series in its library
  to lock a bare `- 5`.)
- `No.Year.Movie.1080p.BluRay.x264-GRP` → Radarr returns empty movieTitle/group (it requires a year);
  cellarr parses the title + group.
- `Apocalypse.Now...Redux`, `...Criterion.Collection` → Radarr returns empty edition; cellarr extracts
  "redux"/"criterion".
These are cases where cellarr is *more* permissive. Not gaps; flagged so they're not mistaken for them.
Whether to match the originals' conservatism (require year for movies, library for bare-absolute) is a
**design choice** — tracked, deferred.

---

## Cross-cutting follow-ups
- **Quality vocabulary alignment:** confirm cellarr's full quality-name set matches the originals'
  (Bluray-1080p, WEBDL-2160p, HDTV-720p, SDTV, Remux tiers, …). G1/G2 closed the resolution/source
  defaulting; a full vocabulary audit is a separate task.
- **Edition vocabulary:** decide cellarr's canonical edition strings vs Radarr's; currently we capture
  the phrase but exact wording can differ.
- **Grow the input corpus:** 120 titles is a starting sample. The originals' real fixture suites are
  ~1,500–2,000 rows; widening cellarr's corpus toward that is the path to a trustworthy parity number.
