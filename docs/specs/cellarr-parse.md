# Spec: cellarr-parse

## Responsibility
Turn a release/file **name** into a `ParsedRelease` with per-field confidence. The most
domain-knowledge-dense crate. Built clean-room against `/corpus/parse` and validated by the
differential oracle. Extraction only — *mapping* anime absolute numbers to season/episode happens at
Identify (`cellarr-media`) using scene-mapping data.

## Allowed dependencies
Internal: `cellarr-core`. External: `regex`, `fancy-regex` (only where lookaround is unavoidable),
`rayon` (batch parsing), `once_cell`/`std::sync::LazyLock`, `serde`. Optional: `cellarr-llm` behind a
feature for the fallback.

## Public interface
- `parse_title(&str) -> ParsedRelease` — the deterministic fast path.
- `parse_batch(&[&str]) -> Vec<ParsedRelease>` — rayon-parallel for search-time bursts.
- Per-extractor functions (resolution, source, codec, audio, hdr, edition, language, group,
  proper/repack, year, numbering) — individually testable.
- A confidence accessor so callers (and the pipeline) can gate on low confidence.

## Behavior
- **Never panics** on arbitrary input (property-tested).
- Deterministic; same input → same output.
- Regexes compiled once; multi-pattern phases use `RegexSet`. Prefer multi-pass over lookaround;
  use `fancy-regex` only where required and adversarially test those patterns.
- Inference fallback (`cellarr-llm`) consulted only when aggregate confidence < threshold; results
  cached by normalized title; **never** authoritative for destructive Import without a confidence
  gate. Local-first; offline-capable. See [04-parser.md](../04-parser.md).
- Clean-room: implemented from behavior + corpus, not transcribed from upstream
  ([agents/legal-and-licensing.md](../agents/legal-and-licensing.md)).

## Test obligations
- Corpus suites (`/corpus/parse/*`) pass at the milestone parity threshold.
- Differential-oracle parity not decreased; intentional divergences allow-listed with rationale.
- `proptest`: no panics on arbitrary input; idempotence where applicable.
- Each extractor has focused unit tests for its slice.
- New edge cases discovered → new corpus vectors in the same change.

## References
[04-parser.md](../04-parser.md), [11-testing.md](../11-testing.md),
[13-upstream-repos.md](../13-upstream-repos.md).
