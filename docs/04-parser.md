# 04 — The release-name parser

The parser turns a release string like
`The.Show.S02E15.1080p.BluRay.x264-GROUP` into structured facts. It is the single most
domain-knowledge-dense component in the project — a decade of edge cases live in the originals'
parsers. Our strategy: **mine their test fixtures as a corpus, write a fresh clean-room parser,
and measure parity against a differential oracle.**

## What "stand on shoulders" means here (and what it doesn't)

- **DO** extract the upstream `[TestCase]` vectors (input → expected fields) into a
  language-neutral corpus. Individual input→output vectors are close to uncopyrightable facts.
- **DO** read the upstream parser to understand *behavior* and the *shape* of the problem.
- **DO NOT** transcribe the upstream regex tables or port `Parser.cs` line by line — that creates a
  derivative work and forces our license. Reimplement clean-room from the corpus + behavior.

See [13-upstream-repos.md](13-upstream-repos.md) for exact file paths, and
[agents/legal-and-licensing.md](agents/legal-and-licensing.md) for the clean-room rules.

## Architecture: extractor pipeline with confidence

The parser is a pipeline of independent **extractors**, each contributing fields to a
`ParsedRelease` with a **per-field confidence**:

- resolution (480p/720p/1080p/2160p…), source (BluRay/WEB-DL/WEBRip/HDTV/DVD/Remux…), video codec
  (x264/x265/HEVC/AV1…), audio (DTS/TrueHD/Atmos/AAC…), HDR flags (HDR10/DV…), edition
  (Director's Cut/Extended/IMAX…), language(s), release group, proper/repack, year, and **numbering**
  (`Coordinates` from [02-data-model.md](02-data-model.md)): single/multi/season/daily/absolute(anime).

Design notes:
- Use `regex::RegexSet` for the multi-pattern phases ("which of these N source patterns match")
  to evaluate many patterns in one linear pass.
- Compile all regexes once (`OnceCell`/`LazyLock`).
- Each extractor is independently unit-testable against its slice of the corpus.
- The orchestration (order of extraction, fallback chains, post-processing fix-ups) is *most of
  the correctness* — the regexes alone are roughly a third of the behavior. Treat the corpus, not
  the regex list, as the spec.

## The regex caveat (read before porting any pattern)

Rust's standard `regex` crate **does not support lookaround or backreferences** — by design, which
is what guarantees linear-time matching and immunity to catastrophic backtracking on hostile
release names. Some upstream patterns rely on lookahead (e.g. matching a year `2019` but not
`1080p`). Therefore:

- **Restructure** most such patterns into multi-pass extraction (often cleaner anyway).
- Fall back to the **`fancy-regex`** crate only for the few patterns that genuinely need lookaround,
  accepting that it reintroduces backtracking — and add adversarial-input tests for those.

This is a *port*, validated by the corpus — never a copy-paste.

## The corpus

Lives in [`/corpus`](../corpus/). Language-neutral (TOML/JSON) vectors, grouped by concern, mirroring
how the upstream fixtures are split:

```
corpus/
  parse/
    single_episode.toml      multi_episode.toml     daily_episode.toml
    season.toml              absolute_anime.toml     miniseries.toml
    movie_title.toml         movie_year.toml         movie_edition.toml
    quality.toml             language.toml           release_group.toml
    unicode.toml             proper_repack.toml
  scoring/                   # see 05-decision-engine.md
  naming/                    # see cellarr-fs spec
```

Each vector is `{ input, expected: {…fields…}, source, notes? }`. The `source` records provenance
(e.g. "derived from Sonarr SingleEpisodeParserFixture"). Re-curate and re-order — do not copy whole
fixture files verbatim (that risks compilation copyright; see legal doc).

**The corpus is the crown jewel.** It is built *before* the parser and it is the parser's
acceptance test. An agent assigned the parser starts by populating the corpus from upstream, gets
it reviewed, then makes vectors pass.

## The differential oracle

In addition to the static corpus, a CI harness runs **real Sonarr/Radarr** (in Docker, via their
APIs / parser test endpoints) over a large set of release titles and **diffs** their output against
cellarr's. This:

- Catches behaviors the static corpus missed (the upstream tests are not exhaustive).
- Turns "parity %" into a number we watch climb.
- Lets us refactor the parser fearlessly. Full mechanism in [11-testing.md](11-testing.md).

Where cellarr *intentionally* differs from upstream, the divergence is recorded as an explicit
allow-listed exception with a rationale — never a silent mismatch.

## Anime numbering (the hardest correctness problem)

Anime releases use **absolute** episode numbers (`Show - 1071`) while the library is addressed by
season/episode. Three numbering systems disagree (scene, TVDB, AniDB). Reconciliation uses **scene
mappings** (TheXEM) and the **anime-lists** AniDB↔TVDB data, resolved in the metadata layer
([07-metadata-service.md](07-metadata-service.md)) and applied during Identify. Budget real time
here; corpus it heavily with `absolute_anime.toml`. The parser's job is to *extract* the absolute
number and group; the *mapping* happens at Identify with the series' scene-mapping data.

## Inference fallback (`cellarr-llm`)

When aggregate confidence is below a threshold, the parser may consult `cellarr-llm` for a
structured parse:

- **Local-first.** Must work with a local model (e.g. llama.cpp/Ollama). A cloud provider is an
  optional enhancement, never a requirement — the offline non-negotiable applies.
- **Cached** by normalized title, so any given weird title costs at most one inference ever.
- **Never authoritative for destructive Import** without a confidence gate (see [03-pipeline.md](03-pipeline.md)).
- Inference results that get confirmed by a successful import should be considered for promotion
  into the corpus (with review), so the deterministic parser keeps improving.

The deterministic parser is always the fast path; inference never replaces it. See
[`specs/cellarr-parse.md`](specs/cellarr-parse.md) and [`specs/cellarr-llm.md`](specs/cellarr-llm.md).
