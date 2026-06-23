# Decision-engine parity — assessment

The decision engine (quality profiles + custom-format scoring + the grab/upgrade/reject precedence)
is harder to oracle than the parser, because the originals don't expose a single "what would you
decide?" endpoint. This file records what's measurable, what isn't, and the plan.

## What the apps expose
`GET /api/v3/parse` already returns, besides the parsed fields:
- `parsedEpisodeInfo.quality.quality.name` / `parsedMovieInfo...` — the **quality bucket**.
- `customFormatScore` and `customFormats[]` — the **CF score**, but only for the custom formats
  *configured in that app instance* (none by default).

There is **no** endpoint for the actual download decision (grab vs upgrade vs reject vs cutoff-met);
that logic runs only during a live search/grab against configured indexers + a populated library.

## Measured
1. **Quality-bucket parity** — covered by the parser oracle: **98.3%**
   ([PARITY_REPORT.md](PARITY_REPORT.md)). Open items are vocabulary, not logic (G7 Remux naming,
   G8 BR-DISK) — see [quality-vocab.md](quality-vocab.md).
2. **Custom-format MATCHING parity — RUN: 100% (120/120) after a fix.** Harness:
   `cellarr-decide/tests/oracle_cf.rs` (run via `just oracle-cf`). One CF set (8 ReleaseTitle-regex
   formats covering repack/proper, x265/HEVC, remux, atmos, DV/HDR, AMZN, MULTi, 10bit) is configured
   in a live Sonarr **and** imported into cellarr via `import_trash_custom_formats`; we diff the
   matched-CF set per corpus title.
   - **First run: 78.3% (94/120).** Every mismatch was `cellarr=∅, sonarr=[CF]` on UPPERCASE tokens
     (HEVC, REPACK, AMZN, HDR, Atmos, MULTi).
   - **Root cause — G-CF1 (high impact):** cellarr matched CF ReleaseTitle regexes **case-sensitively**,
     while Sonarr/Radarr compile CF regexes with **IgnoreCase**. TRaSH CFs are written lowercase and
     rely on this — so cellarr would have matched almost no real-world CFs and made **wrong grab
     decisions**. This is exactly the kind of silent, high-blast-radius bug the oracle exists to catch.
   - **Fix:** compile CF title regexes with `RegexBuilder::case_insensitive(true)`
     (`cellarr-decide::matching`) + a regression unit test. **Re-run: 100% (0 mismatches).**
3. **Custom-format SCORE parity** — once matching is exact (it is) and the quality profile assigns the
   same per-CF scores, score parity follows from matching parity (score = Σ matched-CF scores). A
   direct score oracle (configure a profile with scores both sides, diff `customFormatScore`) is the
   small remaining confirmation; matching — the hard part — is done.

## Full-corpus, real-TRaSH-set CF oracle (matching + score)
The hand-written 8-CF oracle above is a sharp regex probe but tiny. The
`oracle_trash_cf` harness (`tests/oracle_trash_cf.rs`, run via
`just oracle-trash-cf`) is the heavy version: it POSTs the **entire** TRaSH
Sonarr CF set into a live Sonarr and the Radarr set into a live Radarr, imports
the identical sets into cellarr, and diffs — per corpus title, **routed by path**
to the right app (movie_* + upstream/radarr → Radarr; episode/season/anime/daily
+ upstream/sonarr → Sonarr) — both the matched-CF *set* and the CF *score*. It
asserts ratchet floors (gated on `CELLARR_ORACLE_*`, like `oracle_cf`); a static
counterpart (`tests/trash_cf_static.rs`) pins the mechanical behaviors in the
hermetic suite.

### Mechanical gaps found and fixed (this is what the oracle is for)
Running it against the full set surfaced three high-blast-radius, *mechanical*
matching bugs — each verified against the live apps' `/api/v3/parse`:

- **G-CF2 — `ReleaseGroupSpecification` is a regex, not exact-equality.** The
  apps compile the spec's `value` as a case-insensitive regex against the parsed
  release group. cellarr compared it for exact string equality, so e.g.
  `No-RlsGroup` (ReleaseGroup `.` negated = "has no group") matched almost
  everything. Fix: `cellarr-decide::matching` compiles + evaluates ReleaseGroup
  as a regex against `parsed.group` (absent group ⇒ no match before negate).

- **G-CF3 — `SourceSpecification` enum indices are app-specific.** Sonarr and
  Radarr use **different** `QualitySource` enums (verified live): Sonarr
  `television=1, web=3, webRip=4, dvd=5, bluray=6, blurayRaw=7`; Radarr
  `cam=1, telesync=2, telecine=3, workprint=4, dvd=5, tv=6, webdl=7, webrip=8,
  bluray=9`. Index `7` means *blurayRaw* on Sonarr but *WEB-DL* on Radarr — a
  single shared mapping silently mis-matched a whole class of CFs. Fix: the
  importer takes a `TrashApp` dialect (`import_trash_custom_formats*_for_app`);
  `source_from_index` is dialect-specific.

- **G-CF4 — CF boolean algebra is *implementation-grouped*, not flat-OR.** The
  apps group non-required conditions **by implementation**: within an
  implementation they OR, across implementations they AND; required conditions
  are pure AND (even two of the same implementation do *not* OR). cellarr did a
  flat OR over all non-required conditions, so a "tier" CF listing `Source=web`
  plus a set of release-group regexes matched *every* WEB release, not just those
  whose group was in the list. This was the single biggest divergence (the
  `Anime Web Tier`/`Asian Tier`/… clusters). Fix: `MatchContext::matches` groups
  by `discriminant(ConditionKind)`. Verified live with crafted probe CFs
  (`GROUPTEST`/`ORTEST`/`REQOR`/`REQAND`).

### Measured (pinned fixture commit, linuxserver images)
| app    | titles | match-parity (raw) | modelable-match-parity | score-parity |
|--------|-------:|-------------------:|-----------------------:|-------------:|
| Sonarr |  1165  | 0.030              | **0.592**              | **0.550**    |
| Radarr |   544  | 0.325              | **0.436**              | **0.393**    |

*Before the fixes* score-parity was 0.21 (Sonarr) / 0.11 (Radarr). "Modelable"
parity excludes CFs cellarr can never model (unsupported `implementation`s) from
the app's set, isolating the matching-algebra correctness from the unsupported
tail. Raw match-parity is low mainly because `Single Episode` (a
`ReleaseTypeSpecification`) matches almost every Sonarr title and cellarr can't
model it — so nearly every title has ≥1 unavoidable diff.

### Divergence classes (catalogued, not chased)
The harness tags every mismatch; the remaining tail is, in order:

- **unsupported-spec** *(dominant on Sonarr)* — `ReleaseTypeSpecification`
  (`Single Episode`/`Season Pack`/`Multi-Episode`): there is no field on
  cellarr's parse for release type, so these CFs are skipped on import and can
  never match. `Season Pack` is scored (+10), so it also costs score parity. A
  real gap, but it needs a new parse axis, not a matching fix.
- **language-default** *(the `cellarr-stronger` cluster)* — negated
  `LanguageSpecification` CFs (`Language: Not English`, `Not German…`,
  `Wrong Language`). cellarr treats an *absent* language as "not English" and so
  matches; the apps **default an undetected language to English** (and decode the
  numeric language id), so they don't. Fixing this is parser-coupled (default-to-
  English + a language-id table) and out of scope for the CF layer; catalogued
  here.
- **app-builtin-cf / parser-coverage** — app-only matches on `x264`, `720p`,
  `1080p`, etc. on titles where the apps' parser infers a codec/resolution
  cellarr's parser does not (e.g. anime release-group → 720p heuristics, MULTi/
  TrueFrench codec handling). These are parser-coverage gaps, not CF-matching
  gaps.
- **regex-dialect** — a small residue of CFs whose title regex uses a .NET-only
  construct cellarr's engine can't compile (already skip-and-counted on import;
  see G-CF1 and `trash_fixtures.rs`).

## Hard / needs more than a black-box endpoint
3. **Precedence parity** (quality-rank dominates CF score; upgrade only when both cutoffs unmet;
   proper/repack; hard-negative guards). The apps don't expose this directly. Options, in order of
   cost:
   - **Input-level proxy (cheap):** since precedence is a deterministic function of (quality rank,
     CF score, profile, on-disk state), and we can oracle quality rank (#1) and CF score (#2), we can
     argue precedence correctness from validated inputs + cellarr's own precedence unit tests
     (`cellarr-decide` `surprising_rules`). This is what we rely on today.
   - **Live-search oracle (expensive):** drive the app's `/api/v3/release` manual search against a
     test indexer with crafted releases and read which it flags as the chosen grab/upgrade. Requires
     standing up a fake indexer the apps accept. Deferred.
   - **Source study (clean-room caution):** reading the originals' decision code risks the clean-room
     boundary ([../agents/legal-and-licensing.md](../agents/legal-and-licensing.md)); we rely on the
     documented TRaSH semantics instead.

## Identify / matching parity (separate surface)
The parser oracle compares *parsed fields only*. Whether a parse maps to the right series/movie
(cellarr-media's job, incl. anime absolute→episode via scene mappings) needs a **populated library**
in the apps to compare against — a distinct oracle (configure the same series/movies in both, compare
`/api/v3/parse`'s matched `episodes`/`movie`). Designed, not built. Tracked here so it isn't lost.

## Summary of what's left for full decision parity
- [x] **CF-matching oracle: RUN → 100%** after the case-insensitivity fix (G-CF1).
- [ ] CF-score confirmation: assign per-CF scores in a profile both sides, diff `customFormatScore`
  (follows from matching; small).
- [ ] Quality vocabulary alignment (G7/G8) so quality-name parity is 100%, not 98.3%.
- [ ] Precedence: keep input-level proxy; optionally build the live-search oracle later.
- [ ] Identify/matching oracle (needs populated libraries + metadata keys — see roadmap Phase E).
