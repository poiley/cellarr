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
