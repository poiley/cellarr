# Parity report

Measured parser parity of cellarr vs live Sonarr/Radarr. Regenerate with `just oracle`
(raw data: `target/parity/parser-results.json` + `parser-mismatches.jsonl`).

## Run metadata (latest)
- Date: 2026-06-23
- Sonarr **4.0.17.2952** (`lscr.io/linuxserver/sonarr@sha256:02bc962946fef994e67a38152446df25c10a52f8583aefeeb6467f9dd44cab99`)
- Radarr **6.2.1.10461** (`lscr.io/linuxserver/radarr@sha256:1e95b5c13fe015361a9ae1c4d99fc2336816790aaea60fa74b2ffebe076a69e0`)
- Inputs: 120 titles (the `corpus/parse/*.toml` set)
- Surface: `GET /api/v3/parse`

## Headline
**Exact-match rate: 108 / 120 = 90.0%** (every category-relevant field matches).
Started at 76.7% (92/120) before fixes. 17 field-level mismatches remain; of those, ~5 are cases
where **cellarr is stronger** than the originals (they return ∅), ~4 are an intentional edition
**canonicalization** choice, leaving a small set of genuine, mostly-deferred gaps (G3/G4/G7/G8).

> The curated set has since grown to **131 titles**; in the at-scale full-corpus run below it scores
> **119 / 131 = 90.8%** (curated quality nudged to 98.5% after the harness Radarr-face remux fix).
> The full at-scale (curated + upstream) measurement is in
> [At-scale live oracle](#at-scale-live-oracle-full-corpus-curated--upstream).

## Field parity (latest run)
| Field | Parity | Notes |
|-------|--------|-------|
| year | 100.0% (28/28) | — |
| daily_date | 100.0% (5/5) | daily air-date extraction matches Sonarr |
| group | 98.3% (118/120) | remaining 2 = oracle ∅ (cellarr stronger) |
| quality | 98.3% (118/120) | remaining 2 = Remux naming (G7) + BR-DISK bucket (G8) |
| season | 97.0% (32/33) | remaining 1 = miniseries Part-N (G3) |
| episodes | 97.0% (32/33) | remaining 1 = miniseries Part-N (G3) |
| title | 96.7% (116/120) | remaining 4 = G3, G4, + 2 oracle-∅ (cellarr stronger) |
| edition | 78.6% (22/28) | canonicalization divergence (intentional) + 2 oracle-∅ |
| absolute | 87.5% (7/8) | remaining 1 = oracle ∅ on a bare `- 5` (cellarr stronger) |

## Progress across runs
| Run | Change | Exact | quality | season | group | edition |
|-----|--------|-------|---------|--------|-------|---------|
| 1 | baseline | 76.7% | 90.8% | 71.7%* | 97.5% | 75.0% |
| 2 | G1+G2 quality defaults, G6 group hyphen | 78.3% | 98.3% | 71.7%* | 97.5% | 75.0% |
| 3 | G6 hyphen regression fix, G5 Final Cut | 80.0% | 98.3% | 71.7%* | 98.3% | 78.6% |
| 4 | harness: drop daily/anime season sentinel + route movie-CAM to Radarr | **90.0%** | 98.3% | 97.0% | 98.3% | 78.6% |

\* runs 1–3 counted Sonarr's season-0 sentinel for daily/anime as mismatches; run 4's harness
correctly compares daily air-date / absolute number instead (those were never cellarr bugs — see
[parser-gaps.md](parser-gaps.md) A1). cellarr's *parser* did not change for season between runs 3–4.

## What changed in cellarr (fixes this effort)
- **G1** resolution-only releases → `HDTV-<res>` (was `Unknown`). `cellarr-core::resolve_quality`.
- **G2** source-only `HDTV` / sub-HD → `SDTV`. Same.
- **G5** "Final Cut" edition recognized (was missed). `cellarr-parse::edition`.
- **G6** hyphenated release groups captured whole (`D-Z0N3`), with source-tag bleed guard
  (`WEB-DL-GRP` → `GRP`). `cellarr-parse::group`. **Also corrected a wrong corpus expectation**
  the oracle exposed (`release_group.toml` expected the truncated `Z0N3`).

## Upstream self-parity (static, no Docker)

In addition to the live oracle above, a static test measures cellarr against the **full harvested
upstream corpus** (`corpus/upstream/**/*.toml`, 1,555 re-curated input→expected fact vectors) in
plain `cargo test` — `crates/cellarr-parse/tests/upstream_parity.rs`. It writes
`target/parity/upstream-selfparity.json` + `upstream-mismatches.jsonl` and **asserts the overall
field pass-rate ≥ a ratchet** (`RATCHET_OVERALL = 0.65`).

| | overall field rate | exact cases | resolution | source | group | numbering | title |
|--|--|--|--|--|--|--|--|
| baseline | 55.7% | 647/1555 (41.6%) | 80.8% | 64.6% | 26.0% | 54.5% | 51.9% |
| after fixes | **65.6%** | 880/1555 (56.6%) | 86.3% | 73.3% | 71.4% | 58.9% | 56.3% |

**Ratchet:** `0.65` (just under the achieved 0.6564). Raise it as the parser improves; never lower
it to make CI pass. The remaining gap to 100% is mostly intentional divergence (edition
canonicalization), a thin language extractor, and upstream quality-bucket-in-source-field
representation quirks — all catalogued in [parser-gaps.md](parser-gaps.md#upstream-self-parity-static-corpus-measurement).

This pass added the test and fixed the mechanical gaps it surfaced: group repost-suffix peeling +
final-bracket/paren capture, source/resolution run-together aliases + WxH dimensions, multi-episode
numbering variants, and title-cut markers for anime absolutes / release-modifier tags. The curated
`corpus/parse/*.toml` set remains the 100%-must-pass acceptance test (unchanged). Proptest no-panic
remains intact (the parser never panics on any upstream input).

## At-scale live oracle (full corpus: curated + upstream)

The differential oracle now runs the **whole corpus** — the curated `corpus/parse/*.toml` set **and**
the harvested `corpus/upstream/**` set — against the *live* pinned Sonarr/Radarr, routed by path
(`upstream/sonarr/**` → Sonarr, `upstream/radarr/**` → Radarr; curated set routed per-file as
before). It issues calls concurrently (24-way pool, ≤10s/call) so the full set finishes in ~2s wall;
results land in `target/parity/oracle-fullscale.json` + `oracle-fullscale-mismatches.jsonl`.
Run via `just oracle` (now covers the full set); harness is
`crates/cellarr-parse/tests/oracle.rs`.

- Sonarr **4.0.17.2952**, Radarr **6.2.1.10461** (same pins as above), no populated library.
- **1,686 titles** total (131 curated + 1,555 upstream).

### Headline (at scale)
| Partition | Titles | Exact-match | Notes |
|-----------|-------:|:-----------:|-------|
| **ALL** | 1,686 | **720 / 1,686 = 42.7%** | every category-relevant field matches the live app |
| curated (`corpus/parse`) | 131 | **119 / 131 = 90.8%** | matches the curated-only run above |
| upstream (`corpus/upstream`) | 1,555 | **601 / 1,555 = 38.6%** | the harvested originals' own fixtures, run live |

### Per-field parity (ALL / upstream)
| Field | ALL | upstream | dominant residual class |
|-------|----:|---------:|--------------------------|
| year | 98.8% | 98.8% | — |
| quality | 91.6% | 91.0% | resolution-bucket interactions; G7 remux now face-normalised (no longer false) |
| edition | 91.8% | 92.6% | canonicalization divergence (intentional, G5) |
| group | 76.6% | 74.8% | multi-token / inner-paren-precedence groups + **library-locked oracle-∅** |
| season | 76.3% | 74.1% | live-app library-lock; Part-N (G3) |
| daily_date | 73.5% | 69.0% | compact `YYMMDD`/`YYYYMMDD`, `MM.DD.YYYY`, `DD-MM-YYYY` forms (deferred) |
| episodes | 69.8% | 67.0% | scene concat numbering + library-lock |
| title | 63.2% | 60.4% | G4 year/season-suffix, `(YEAR)`-bracket cleaner, **library-locked oracle-∅** |
| absolute | 55.6% | 53.9% | `#NNN`/mid-title bare absolute (deferred) + library-lock |

### The dominant at-scale finding: the live oracle is *weaker* than its own fixtures
Of the **1,476** field mismatch rows, ~**26% (384) are `oracle-∅`** — cellarr produced a value the
live app left **empty** (226 group, 145 title). This is the **A3 phenomenon at scale**: the
re-curated upstream vectors are facts the originals assert in *unit tests with a mocked series in the
library*, but the live `/api/v3/parse` endpoint with **no populated library cannot lock** anime
series titles, library-keyed groups, or bare-absolute numbering, so it returns blanks where cellarr
(and the curated fact) parse successfully. These are **cellarr-stronger / library-locked**, not
gaps. For library-dependent fields the **static upstream self-parity below is the truer reference**
(it compares against the re-curated fact, deterministically, no library needed).

Breakdown of the 1,476 mismatch rows by direction: **384 oracle-∅** (cellarr stronger / library-lock)
· **413 cellarr-∅** (cellarr empty — mix of deep group multi-token, `#`-form absolutes, library-lock)
· **681 both-non-∅** (genuine value disagreement — title G4/`(YEAR)` 376, quality resolution-bucket
142, group multi-token 78).

### Mechanical fixes landed this pass (re-measured)
- **Anime bare-absolute title cut.** cellarr captured the absolute coordinate but **left the number
  in the title** (`Show One 07`, `Series.Title.100`, `Title-01`). Added a fansub-context title cut
  (`crates/cellarr-parse/src/title.rs`): only when a leading `[Group]` bracket was stripped **and**
  the numbering layer already found an `Absolute`, so a number that is part of a real title
  (`Apollo 13`) is never severed. Static upstream title 56.3 → 56.8%.
- **ISO / dash daily dates.** `normalize` leaves `-`, so `Series - 2013-10-30 - …` and
  `2018-11-14.…` were missed (only space-separated `YYYY MM DD` matched). Widened the daily regex to
  accept space **or** hyphen between parts (`crates/cellarr-parse/src/numbering.rs`). Live daily_date
  58.8 → **73.5%**; static upstream overall 65.6 → **65.9%**.
- **Harness: Radarr-face remux name.** The oracle now renders cellarr's canonical `bluray-<res>
  remux` as `remux-<res>` for Radarr-routed titles (mirroring the `/api/v3` shim's
  `face_quality_name`), so the **known, intended** G7 vocabulary difference is no longer counted as a
  false mismatch. Live quality 90.0 → **91.6%**; ALL exact 41.5 → **42.7%**.

### Allow-list (intentional divergence / oracle artifact at scale — not cellarr bugs)
- **Library-locked oracle-∅** (A3) — 384 rows. Live app returns ∅ without a populated library;
  cellarr parses the curated fact. Reference is the static set, not the live blank.
- **Edition canonicalization** (G5) — cellarr normalises edition spelling; upstream preserves the raw
  phrase. ~92% match; the residual is the canonical-spelling choice.
- **Remux face spelling** (G7) — now normalised in the harness; cellarr is correct on both faces.
- **Quality bucket-in-source representation** — upstream curators recorded the derived *bucket* name
  in the `source` field (`HDTV`+no-res→`SDTV`, `480i REMUX`→`bluray`); cellarr keeps the literal
  source and derives the bucket in `resolve_quality`. Consistent with G1/G2.

### Still-real, deferred (long tail — not chased this pass)
- **Title G4** (disambiguating year / anime season-suffix kept by upstream: `Watchmen 2019`,
  `… S2`) and the leading `(YEAR)`-bracket cleaner gap — needs targeted, corpus-backed changes.
- **Compact/region daily forms** (`140722`, `20201013`, `04.28.2014`, `30-04-2024`) — ambiguous with
  absolute/large numbers; needs guarded rules.
- **`#NNN` and mid-title bare absolutes** (`Series Title #957`, `Show 01 Role Play`) — numbering
  doesn't recognise these, so the title cut (gated on a found `Absolute`) correctly does not fire.
- **Deep group multi-token / inner-paren precedence** (`H264-BEN.THE.MEN` → `men`,
  `(… Tigole) [QxR]` → `qxr`) — multi-word group + precedence; risk-gated.

## Remaining gaps
See [parser-gaps.md](parser-gaps.md) for the full catalogue and classification. Decision-engine
parity is assessed separately in [decision-gaps.md](decision-gaps.md).
