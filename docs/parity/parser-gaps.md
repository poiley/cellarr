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
- **Status: FIXED (vocabulary).** One canonical internal name kept (`Bluray-<res> Remux`, the Sonarr
  spelling); the `/api/v3` shim's `face_quality_name` renames it to `Remux-<res>` on the Radarr face.
  Verified against both live apps and pinned by `qualitydefinition_remux_name_differs_per_face`. See
  [quality-vocab.md](quality-vocab.md).

### G8 — No full-disc (BR-DISK) quality bucket
`Movie.2019.4K.UHD.BluRay.2160p.HDR.HEVC-GRP` (no remux, no encode marker) → Radarr `BR-DISK` (raw
disc); cellarr → `Bluray-2160p`. cellarr has no full-disc bucket, so it collapses discs into the
encoded Bluray tier.
- **Status: FIXED (real, niche).** Added `BR-DISK` and `Raw-HD` quality buckets (plus the Radarr
  pre-retail movie tiers) to the default ranking, backed by new `Source` variants the parser detects:
  `BR-DISK`/`COMPLETE.BLURAY`/`BDMV`/`M2TS`/`BD25`/`BD50`/`UHD-BD` (full disc, listed before the
  encoded-Bluray pattern so a raw disc wins), and `Raw-HD`/`MPEG-TS` (untouched HD capture). Note the
  original G8 example `…4K.UHD.BluRay.2160p.HDR.HEVC…` carries an encode marker (`HEVC`), so it stays
  `Bluray-2160p`; only structure tokens *without* an encode bucket to `BR-DISK`. Corpus vectors added.
  See [quality-vocab.md](quality-vocab.md).

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

### G9 — Anime bare absolute left in the clean title ⬅ at-scale (full corpus)
Surfaced by the full-corpus oracle. For fansub forms where the absolute number is glued to the title
(no ` - ` separator) — `[SubDESU]_Show_One_07_…`, `[Hatsuyuki]Series_Title-01…`,
`[DRONE]Series.Title.100` — cellarr captured the `Absolute` coordinate but **left the number in the
title** (`Show One 07`), while Sonarr cuts the title at the number (`Show One`).
- **Status: FIXED** (`cellarr-parse::title`). A fansub-context title cut now severs a trailing bare
  absolute number, **gated** so it only fires when (a) a leading `[Group]` bracket was stripped
  (anime context) **and** (b) the numbering layer already produced an `Absolute` — so a number that
  is genuinely part of a title (`Apollo 13`, `District 9`) is never cut. Static upstream title
  56.3→56.8%. The `#NNN` and mid-title (`Show 01 Role Play`) forms remain (numbering doesn't yet
  recognise them, so the gated cut correctly does not fire) — deferred long tail.

### G10 — ISO / dash-separated daily dates missed ⬅ at-scale (full corpus)
`normalize` collapses `.`/`_`/space but leaves `-`, so the daily regex (space-separated `YYYY MM DD`)
missed `Series - 2013-10-30 - …`, `A Late Talk Show - 2011-04-12 - …`, `2018-11-14.…`.
- **Status: FIXED** (`cellarr-parse::numbering`). The daily date regex now accepts a space **or** a
  hyphen between the year/month/day parts (`[\s-]`), with the same month 01–12 / day 01–31 plausibility
  guard. Live daily_date 58.8→73.5%; static upstream overall 65.6→65.9%. Compact (`YYMMDD`,
  `YYYYMMDD`) and region-ordered (`MM.DD.YYYY`, `DD-MM-YYYY`) forms remain deferred (ambiguous with
  absolute/large numbers; need guarded rules).

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

### A4 — At-scale: the live oracle is *weaker* than its own fixtures (library-locked oracle-∅)
Running the full upstream corpus (1,555 titles) against the *live* apps, **384 of 1,476 mismatch rows
(~26%) are `oracle-∅`** — cellarr produced a value the live app left **empty** (226 group, 145 title).
The upstream vectors are facts the originals assert in unit tests *with a mocked series in the
library*; the live `/api/v3/parse` with **no populated library cannot lock** anime series titles,
library-keyed groups, or bare-absolute numbering, so it blanks them while cellarr (and the curated
fact) parse them. This is the [A3](#a3--originals-return--where-cellarr-parses-successfully-cellarr-stronger)
phenomenon at scale: **cellarr-stronger / library-locked, not gaps.** For these library-dependent
fields the **static `upstream_parity.rs`** (compares against the re-curated fact, deterministically,
no library) is the truer reference than the live blank. Recorded so the at-scale upstream exact-rate
(38.6%) is read correctly — a large slice of the gap to the curated 90.8% is this artifact plus the
intentional divergences below, not regressions.

---

---

## Upstream self-parity (static corpus measurement)

Separate from the live differential oracle, `crates/cellarr-parse/tests/upstream_parity.rs`
measures cellarr against the full harvested upstream corpus (`corpus/upstream/**/*.toml`, 1,555
re-curated input→expected fact vectors). It runs in plain `cargo test` (no Docker), writes
`target/parity/upstream-selfparity.json` + `upstream-mismatches.jsonl`, and **asserts the overall
field pass-rate ≥ a ratchet floor** (`RATCHET_OVERALL`, currently `0.65`). The floor is the achieved
rate, not 100% — the upstream set is the originals' own fixtures, which encode behaviors cellarr
deliberately diverges from. Raise the ratchet as fixes land; never lower it to make CI green.

**Achieved this pass:** overall field rate **65.6%** (up from **55.7%** baseline), 880/1,555 cases
exact. Per-field after fixes: year 96.3 · resolution 86.3 · source 73.3 · group 71.4 · numbering 58.9
· title 56.3 · languages 37.2 · edition 26.8.

### Fixes landed this pass (mechanical, clean-room)
- **Group +45pts (26.0→71.4%).** (a) Repost/obfuscation suffix peeling — a re-curated *fact list*
  of scene re-upload tags (`Rakuv`, `postbot`, `xpost`, `Obfuscated`, `NZBgeek`, `RP`, …) stripped
  after the real group (`EVO-Rakuv`→`EVO`). (b) Final `[GROUP]` bracket (`x264-[YTS.MX]`, `[HDO]`,
  `[QxR]`). (c) Group trailing inside the x265 quality parens (`(… 10bit AAC 7.1 Tigole)`).
  (d) Dot-separated trailing group (`…MA.5.1.KRaLiMaRKo`). (e) Trailing site-tag tolerance
  (`-2HD [eztv]-[rarbg.com]`→`2HD`).
- **Source +8.7pts (64.6→73.3%).** Run-together Bluray spellings (`MBluRay`/`BDLight`/`BDMux`/
  `BD720p`/`UHDBDRip`/`Bluray1080p`), WEB aliases (`WebHD`/`iTunesHD`/`WEBMux`), CAM family
  (`HQCAM`/`HDCAMRip`/`NEWCAM`), telesync (`TSRip`/`TeleSynch`), `xvidvd`/`nDVDn` DVD forms.
- **Resolution +5.5pts (80.8→86.3%).** Glued forms (`BD720p`, `540p`→480p tier) and explicit
  `WIDTHxHEIGHT` dimensions (`1280x720`, `640x480`) binned by height.
- **Numbering +4.4pts (54.5→58.9%).** Repeated/spaced multi-episode markers (`S03E01.S03E02`,
  `S07E22 - S07E23`, `S6E1-S6E2`), mixed range/list (`E01-02-03`, `E96-97-98-99-100`), multi-NxN
  (`2x04x05`, `2x04.2x05`, `2x01-x02`), word forms (`Season 1 Episode 5-6`, `Sxx EpA-EpB`), and
  foreign season-pack words (`Saison`/`Stagione`/`Temporada`/`Staffel`).
- **Title +4.4pts (51.9→56.3%).** Added `Ep##`/`Episode ##`/`WxH`/foreign-season/edition/dub markers
  to the title-cut alternation so anime absolutes and release-modifier tag soup stop the title.

### Residual divergence classes
- **edition (26.8%) — INTENTIONAL (G5).** cellarr canonicalizes editions to a stable spelling
  (`Directors Cut`→`Director's Cut`, `Extended Cut`→`Extended`) for consistent custom-format
  matching; upstream preserves the raw phrase. The harness compares exact strings, so this reads as a
  mismatch by design. A residual minority are genuinely unrecognized one-off editions
  (`Despecialized`, `Diamond/Signature/Imperial Edition`, `Assembly Cut`) — REAL but niche; adding
  them is low-value vocabulary churn, deferred.
- **languages (37.2%) — REAL but deep.** CJK script detection (`[CHS]`/`[GB]`/`简繁`), Sonarr's
  multi-language precedence and its many two/three-letter abbreviations are a large, separate effort;
  the language extractor is a thin slice today. Tracked, not chased in this mechanical pass.
- **title — partly INTENTIONAL ambiguity.** A bare single-word language (`The Good German`,
  `The French Movie`) is left in the title rather than cut: upstream treats it as a release tag in
  some positions and as part of the title in others, and cellarr can't disambiguate without library
  context. Cutting on it raised the number ~0.5pt but produced wrong titles, so it was reverted —
  correctness over the metric. Anime season suffixes kept in title (`… S2`/`S3`) and the leading
  `(YEAR)` bracket truncation are pre-existing title-cleaner gaps, deferred.
- **source — upstream resolution-interaction quirks.** `HDTV`+no-resolution→`SDTV`, low-res
  (`480i`) `REMUX`→`bluray`, `HDTV`+`MPEG2`→`raw-hd`, `Bluray ISO`/`BD-50`→`br-disk` are
  resolution/codec-dependent *quality-bucket* rules. cellarr keeps the literal `source` (e.g. `hdtv`)
  and lets `resolve_quality` derive the bucket; the upstream curators recorded the *bucket* name in
  the `source` field, so these read as source mismatches that are really quality-name representation
  differences (consistent with G1/G2). Tracked; not a literal-source bug.
- **numbering — scene 3/5-digit concat.** `103.104`→S1[3,4], `10708`→S1[7,8] (Sonarr's
  `SeasonEpisodePatterns` for concatenated numbers) are ambiguous with absolute/year and were left
  out to avoid false positives. REAL but risk-gated.

## Cross-cutting follow-ups
- **Quality vocabulary alignment:** confirm cellarr's full quality-name set matches the originals'
  (Bluray-1080p, WEBDL-2160p, HDTV-720p, SDTV, Remux tiers, …). G1/G2 closed the resolution/source
  defaulting; a full vocabulary audit is a separate task.
- **Edition vocabulary:** decide cellarr's canonical edition strings vs Radarr's; currently we capture
  the phrase but exact wording can differ.
- **Grow the input corpus:** 120 titles is a starting sample. The originals' real fixture suites are
  ~1,500–2,000 rows; widening cellarr's corpus toward that is the path to a trustworthy parity number.
