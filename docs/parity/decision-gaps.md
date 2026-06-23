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

## Measurable now (cheap, high value) — NOT yet run
1. **Quality-bucket parity** — already covered by the parser oracle: **98.3%**
   ([PARITY_REPORT.md](PARITY_REPORT.md)). The two open items are vocabulary, not logic
   (G7 Remux naming, G8 BR-DISK) — see [parser-gaps.md](parser-gaps.md).
2. **Custom-format score parity** — feasible and the clear next step:
   - Import a known **TRaSH-Guides** CF set into Sonarr/Radarr via `POST /api/v3/customformat`
     (and the matching quality profile), and the **same** set into cellarr (it already has a TRaSH
     importer — `cellarr-decide::trash`).
   - For each release title, compare the app's `customFormatScore` (from `/api/v3/parse`) against
     `cellarr-decide::score(parsed, profile, cfs)`.
   - This directly oracles CF condition matching + scoring — the part of the decision engine most
     likely to diverge, and the part users tune most.
   - **Status: designed, not executed.** With zero CFs configured both sides score 0 (a meaningless
     match), so this needs the CF import step first. Tracked as the next decision-parity task.

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
- [ ] CF-score oracle: import a TRaSH set into apps + cellarr, diff `customFormatScore` over releases. **(next)**
- [ ] Quality vocabulary alignment (G7/G8) so quality-name parity is 100%, not 98.3%.
- [ ] Precedence: keep input-level proxy; optionally build the live-search oracle later.
- [ ] Identify/matching oracle (needs populated libraries).
