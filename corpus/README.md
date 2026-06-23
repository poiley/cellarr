# corpus — the executable domain knowledge

This directory holds **language-neutral test vectors**: the distilled, decade-deep domain knowledge
of the *arr stack, expressed as `input → expected output` facts. It is the **spec** for the
domain-knowledge components and the thing that makes "copy then improve" safe.

Read [../docs/11-testing.md](../docs/11-testing.md) and
[../docs/agents/legal-and-licensing.md](../docs/agents/legal-and-licensing.md) before adding vectors.

## Why it exists
- It is built **before** the components it tests (parser, decision engine, rename engine).
- It is consumed identically by Rust tests (via `rstest` `#[case]`) and by the differential oracle.
- It is how the swarm **compounds** knowledge: when any agent discovers an edge case, it adds a
  vector, and no one ever has to rediscover it.

## Layout
```
corpus/
  parse/      release-name → expected parsed fields (see docs/04-parser.md)
              single_episode  multi_episode  daily_episode  season  miniseries
              absolute_anime  movie_title  movie_year  movie_edition
              quality  language  release_group  unicode  proper_repack
  scoring/    custom-format match / score / decision vectors (see docs/05-decision-engine.md)
  naming/     content + naming tokens → expected on-disk path (see docs/specs/cellarr-fs.md)
  anime/      absolute ↔ season/episode mapping expectations (shared parser + metadata)
```

## Vector format
Vectors are TOML (or JSON) tables. Every vector records its **provenance**. Illustrative shape:

```toml
[[case]]
input    = "Series.Title.S02E15.1080p.BluRay.x264-GROUP"
source   = "derived from Sonarr SingleEpisodeParserFixture"   # provenance, always required
notes    = "release group after final dash"                    # optional
[case.expected]
title      = "Series Title"
season     = 2
episode    = 15
resolution = "1080p"
source_tag = "BluRay"
codec      = "x264"
group      = "GROUP"
```

The exact `expected` field names are owned by `cellarr-core`'s `ParsedRelease` (and the scoring /
naming types). Keep field names in lockstep with those types.

## Rules for adding vectors (clean-room)
1. **Re-curate, don't bulk-copy.** Extract individual `input → expected` facts; do **not** copy a
   whole upstream fixture file verbatim (that would copy its selection/arrangement). Re-order,
   regroup, and merge sources.
2. **Always set `source`.** It documents that we took a *fact*, and enables later audit.
3. **One concern per file**, mirroring how the behavior is split, so failures localize.
4. **Add a vector with every bug fix and every discovered edge case** — this is part of "done."
5. Anything beyond individual factual vectors, or from a source without a clear compatible license,
   is **not** an autonomous decision — flag it (see the legal doc).

## How it's consumed
- **Rust:** each component's test harness loads its corpus dir and runs every case.
- **Differential oracle:** the same inputs are run through the pinned real *arr apps and diffed;
  parity is a tracked CI number ([../docs/11-testing.md](../docs/11-testing.md)).
