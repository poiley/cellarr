# Parity — measuring cellarr against the *arr originals

This directory is the **durable record** of the differential-oracle effort: standing up real
Sonarr/Radarr, diffing their output against cellarr's, measuring parity, and cataloguing **every**
gap we find. Nothing discovered here should be lost — if it's a real difference, it gets written
down in one of these files, even if we don't fix it immediately.

## Why
cellarr passes its own corpus, but that only proves self-consistency. The *value* of the *arr
stack is a decade of release-naming and decision edge cases. The only way to know how close
cellarr is, is to run the real apps over the same inputs and diff. See
[../11-testing.md](../11-testing.md) (the differential-oracle design) and
[../14-roadmap.md](../14-roadmap.md) (the parity thresholds).

## The oracle surface
Both apps expose `GET /api/v3/parse?title=<release>` (auth: `X-Api-Key`) which returns the parsed
release (title, season/episode(s), quality, release group, languages, …) plus any matched
series/movie. That endpoint is the parser oracle. Decision parity is harder (needs configured
profiles + a release to evaluate) — tracked separately in [decision-gaps.md](decision-gaps.md).

## Files
| File | Contents |
|------|----------|
| [oracle-setup.md](oracle-setup.md) | How the oracle is stood up: images/versions, API keys, the parse endpoint, obstacles + workarounds. |
| [methodology.md](methodology.md) | Exactly how we compare (field mapping cellarr↔Sonarr/Radarr, normalization rules, what counts as a mismatch). |
| [parser-gaps.md](parser-gaps.md) | **The catalogue.** Every parser difference found, by category, with examples. The master gap list. |
| [decision-gaps.md](decision-gaps.md) | Decision-engine parity: approach, what's measurable, gaps. |
| [PARITY_REPORT.md](PARITY_REPORT.md) | The measured numbers per category + run metadata. Regenerated each run. |

## Status
- [x] Oracle stood up: pinned Sonarr 4.0.17 + Radarr 6.2.1 (see oracle-setup.md)
- [x] Parser oracle harness built (`cellarr-parse/tests/oracle.rs`, run via `just oracle`)
- [x] Parser parity measured: **90.0% exact** (120 titles), up from 76.7% (PARITY_REPORT.md)
- [x] Parser gaps catalogued + classified (parser-gaps.md): G1/G2/G5-FinalCut/G6 fixed; G3/G4/G7/G8 documented
- [x] Decision-engine oracle assessed (decision-gaps.md): CF-score oracle is the defined next step
- [x] Gaps triaged (fixed mechanical gaps; deferred judgment-call & vocabulary gaps with notes)
- [ ] **Next:** CF-score oracle (import TRaSH set into apps + cellarr, diff scores); quality vocab (G7/G8); identify/matching oracle

_Last updated: 2026-06-23._
