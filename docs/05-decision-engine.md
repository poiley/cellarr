# 05 — The decision engine

The decision engine answers: **given a parsed, identified release and what's already on disk,
should we grab it?** It must be deterministic, explainable, and compatible with the community's
mental model (TRaSH Guides), because that mental model is what users already know.

Lives in `cellarr-decide`. Every verdict it produces is written to the `decision_log`
([03-pipeline.md](03-pipeline.md)).

## The model (TRaSH-compatible semantics)

We mirror the conceptual model of the *arr apps so existing user knowledge and community configs
transfer. We reimplement it clean-room; we do not copy upstream code.

### Quality definitions
A single global, ordered list from worst → best (e.g. SDTV → … → Bluray-2160p Remux), each with a
size range (min/max/preferred per unit or per minute). This is the master ranking.

### Quality profile
Per library (and overridable per item):
- the **allowed** qualities (a subset, with custom grouping and ordering),
- **upgrades allowed** (yes/no),
- the **upgrade-until / cutoff** quality (stop upgrading once reached),
- the **minimum custom-format score** (below this, reject),
- the **upgrade-until custom-format score** (stop chasing CF score once reached),
- language requirements.

### Custom formats (CFs)
Named bundles of **conditions** (specifications). Condition kinds: release-title regex, release
group, source, resolution, quality modifier (REMUX/PROPER/REPACK), language, indexer flag
(e.g. freeleech), size. Semantics:
- conditions **OR** by default,
- `required: true` makes a condition **AND** (must match),
- `negate: true` matches on **absence**.

Each CF carries a **score** (positive, negative, or zero). A release's CF score is the **sum of the
scores of all matching CFs**. Large negatives (e.g. −10000) act as hard "never download" guards.

## The decision function (precedence is the whole game)

Given a candidate release and the current on-disk file (if any):

1. **Hard rejects first.** Disallowed quality, below minimum CF score, blocklisted release,
   failed size constraints, unmet language requirement → reject (with reason).
2. **Quality rank dominates.** Compare quality position in the profile order. cellarr will **not**
   downgrade quality to chase a higher CF score. A better quality wins regardless of CF score
   (within what the profile allows).
3. **CF score breaks ties** within the same quality rank. Higher total CF score wins.
4. **Upgrade gating.** If a file already exists, only grab when it's a genuine upgrade and upgrades
   are allowed and neither cutoff (quality cutoff *and* CF-score cutoff) has been met. Once both
   cutoffs are satisfied, stop — no churn.
5. **Proper/repack** handling per policy (prefer, or only when it fixes the current file).

Every branch records its reason in the decision log. "Rejected: quality DVD not in profile",
"Upgraded over: Bluray-1080p (score 50) → Bluray-1080p Remux (score 120)", etc.

## TRaSH-Guides interoperability

[TRaSH Guides](https://trash-guides.info) publishes community CF definitions (regexes) and
recommended scores as JSON, keyed by profile flavor (`default`/`anime`/`german`, etc.), usually
applied via Recyclarr/Configarr. cellarr should:

- be able to **import** TRaSH-format custom formats and scores directly (so users keep their setup),
- ship sensible defaults derived from community practice,
- keep its CF condition schema a **superset-compatible** match for TRaSH's so imports are lossless.

This is the highest-leverage reuse in the decision layer: we inherit the community's tuning rather
than re-derive it. See [13-upstream-repos.md](13-upstream-repos.md).

## Testing the decision engine

The decision engine is pure and deterministic — ideal for table-driven tests. The corpus lives in
`corpus/scoring/`:

- **CF matching vectors:** `{ release_fields, custom_format, expected_match }`.
- **Scoring vectors:** `{ release, profile, custom_formats, expected_score }`.
- **Decision vectors:** `{ candidate, on_disk, profile, expected_verdict, expected_reason_kind }`.

The differential oracle ([11-testing.md](11-testing.md)) also compares cellarr's grab/reject/upgrade
decisions against the originals' for the same inputs and configs, so decision parity is a measured
number, not an assertion. Precedence rules (quality-over-score, both-cutoffs-to-stop) get explicit
dedicated tests because they are the rules users most often get surprised by. See
[`specs/cellarr-decide.md`](specs/cellarr-decide.md).
