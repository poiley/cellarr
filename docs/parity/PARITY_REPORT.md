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

## Remaining gaps
See [parser-gaps.md](parser-gaps.md) for the full catalogue and classification. Decision-engine
parity is assessed separately in [decision-gaps.md](decision-gaps.md).
