# Spec: cellarr-decide

## Responsibility
The decision engine: given a parsed+identified release and what's on disk, produce a verdict
(grab / upgrade / reject) and a reason. Pure and deterministic. Implements quality profiles and
custom-format scoring with TRaSH-compatible semantics. Clean-room reimplementation.

## Allowed dependencies
Internal: `cellarr-core`. External: `regex`, `serde`, `thiserror`. No I/O.

## Public interface
- `score(release, profile, custom_formats) -> Score` — sum of matching CF scores.
- `matches(custom_format, release_fields) -> bool` — CF condition evaluation (OR default,
  `required`=AND, `negate`=absence).
- `decide(candidate, on_disk: Option<File>, profile) -> Decision` — the verdict + structured reason.
- TRaSH import: `import_trash_custom_formats(json) -> Vec<CustomFormat>`.

## Behavior (precedence is the contract)
1. Hard rejects first (disallowed quality, below min CF score, blocklisted, size/language fails).
2. **Quality rank dominates** — never downgrade quality to chase CF score.
3. CF score breaks ties within equal quality.
4. Upgrade only when it's a real upgrade, upgrades allowed, and **both** cutoffs (quality + CF score)
   unmet; stop once both are met (no churn).
5. Proper/repack per policy.
- Every branch emits a structured reason for the decision log. See [05-decision-engine.md](../05-decision-engine.md).

## Test obligations
- `corpus/scoring/*`: CF-match, scoring, and decision vectors pass.
- Differential-oracle decision parity not decreased.
- Dedicated tests for the surprising rules: quality-over-score, both-cutoffs-to-stop, hard-negative
  guards, proper/repack handling.
- TRaSH import round-trips known community CF sets into equivalent decisions.

## References
[05-decision-engine.md](../05-decision-engine.md), [11-testing.md](../11-testing.md).
