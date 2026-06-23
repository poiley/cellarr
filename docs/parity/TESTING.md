# Testing & test-hardening — the bulletproof state

How cellarr's parser, decision engine, and filesystem layer are tested, and the
hard numbers behind the claim that the safety-critical paths are pinned. Every
figure here is reproduced from a committed artifact or a recorded run — no number
is asserted that the suite does not also assert.

Run the gate with `just ci`; the fast hermetic subset is `just test`. The
differential oracle and the at-scale TRaSH-CF harnesses need Docker
(`just oracle`, `just oracle-trash-cf`) and self-skip when their env vars are unset.

Related parity docs:
[PARITY_REPORT.md](PARITY_REPORT.md) ·
[parser-gaps.md](parser-gaps.md) ·
[decision-gaps.md](decision-gaps.md) ·
[api-v3-gaps.md](api-v3-gaps.md) ·
[methodology.md](methodology.md) ·
[oracle-setup.md](oracle-setup.md) ·
[quality-vocab.md](quality-vocab.md) ·
[README.md](README.md)

---

## 1. The strategy, in layers

The test approach is defense-in-depth: each layer catches a different failure
class, and the layers above the hermetic line run on every `cargo test`.

| Layer | What it proves | Hermetic? | Where |
|-------|----------------|:---------:|-------|
| **Curated corpus** | every hand-pinned fact still parses exactly | yes | `crates/cellarr-parse/tests/corpus.rs` (+ `corpus/parse/*.toml`) |
| **Upstream self-parity** | parser tracks the originals' own fixtures, ratcheted | yes | `crates/cellarr-parse/tests/upstream_parity.rs` (+ `corpus/upstream/**`) |
| **Differential oracle** | parser agrees with live Sonarr/Radarr `/api/v3/parse` at scale | no (Docker) | `crates/cellarr-parse/tests/oracle.rs` |
| **CF match/score oracle** | custom-format matching + scoring equals live apps | no (Docker) | `crates/cellarr-decide/tests/oracle_cf*.rs`, `oracle_trash_cf.rs` |
| **CF static counterparts** | the mechanical CF behaviors caught by the oracle stay fixed | yes | `crates/cellarr-decide/tests/trash_cf_static.rs`, `trash_fixtures.rs` |
| **Property tests** | invariants hold over a randomized space, parser/renamer never panic | yes | `proptest_precedence.rs`, `proptest_naming.rs`, `proptest_no_panic.rs` |
| **Mutation tests** | the tests actually *kill* logic mutations (no fake green) | tool | `crates/cellarr-{parse,decide,fs}/tests/mutation_kills.rs` + `cargo mutants` |
| **Fuzz target** | the parser is a total function on arbitrary bytes | nightly | `crates/cellarr-parse/fuzz/fuzz_targets/parse_title.rs` |

---

## 2. Curated corpus — 100% must-pass

`corpus/parse/*.toml` is the parser's acceptance test: **131 hand-curated case
vectors across 14 files** (each records its provenance in `source`; each vector
pins only the fields it declares). `corpus.rs` aggregates every failing vector
and panics if **any** fail — so a green run is, by construction, **131/131 =
100%**. This set is the contract; it never ratchets down.

## 3. Upstream corpus + ratcheted self-parity

`corpus/upstream/**/*.toml` is the originals' own parser-test fixtures,
re-curated clean-room as input→expected **facts** (see
[legal-and-licensing](../agents/legal-and-licensing.md)): **1,555 case vectors
across 16 files**. `upstream_parity.rs` runs them in plain `cargo test` (no
Docker), computes per-field and overall pass rates, writes
`target/parity/upstream-selfparity.json`, and **asserts an overall-field-rate
ratchet floor of 0.65**.

Last recorded run (`target/parity/upstream-selfparity.json`):

| metric | value |
|--------|------:|
| overall field rate | **0.6594** (1,731 / 2,625 asserted fields) |
| exact-case rate | 0.5678 (883 / 1,555) |
| ratchet floor (asserted) | 0.65 |

Unlike the curated set, this floor is **not** 100% by design: the upstream
fixtures include cases where cellarr deliberately diverges (canonicalized
editions, library-locked anime, Part-N miniseries, a thin language extractor).
The floor is a one-way ratchet — raise it as the parser improves, never lower it.

## 4. Differential oracle at scale (curated + upstream partitions)

`oracle.rs` (`#[ignore]`, run via `just oracle`) diffs cellarr's parser against
the **live** Sonarr/Radarr `/api/v3/parse` endpoints over the **whole** corpus,
routed by path: `corpus/parse/*.toml` (curated, movie-shaped generics → Radarr),
`corpus/upstream/sonarr/**` → Sonarr, `corpus/upstream/radarr/**` → Radarr.
Calls are concurrent with per-call timeouts; results land in
`target/parity/oracle-fullscale.json` + a JSONL of every mismatch so nothing is
lost.

Pinned images: Sonarr **4.0.17.2952**, Radarr **6.2.1.10461** (linuxserver,
digest-pinned in [PARITY_REPORT.md](PARITY_REPORT.md)).

- **Curated partition at scale:** **119 / 131 = 90.8%** exact title-level (recorded
  in `target/parity/parser-results.json` as 118–119/131 depending on the
  Radarr-face remux harness fix; per-field breakdown in
  [PARITY_REPORT.md](PARITY_REPORT.md)). The residual is overwhelmingly
  *cellarr-stronger* (oracle returns ∅) or intentional edition canonicalization.
- **Upstream partition at scale:** thousands of titles; mismatches catalogued by
  class in [parser-gaps.md](parser-gaps.md), not chased blindly.

## 5. Real-TRaSH custom-format parity — and the bugs it caught

Two oracle tiers diff cellarr's CF matching and scoring against the live apps:

**Sharp 8-CF probe** (`oracle_cf.rs` / `oracle_cf_score.rs`, `just oracle-cf`) —
one hand-built CF set imported into a live Sonarr and into cellarr, diffed per
corpus title:

| oracle | result | artifact |
|--------|-------:|----------|
| CF **matching** | **120 / 120 = 100%** | `docs/parity/results/cf-results.json` |
| CF **score** | **131 / 131 = 100%** | `target/parity/cf-score-results.json` |

**Full TRaSH set at scale** (`oracle_trash_cf.rs`, `just oracle-trash-cf`) — the
*entire* TRaSH Sonarr CF set POSTed into a live Sonarr and the Radarr set into a
live Radarr, the identical sets imported into cellarr, diffed per title routed by
path. Recorded (`target/parity/trash-cf-results-{sonarr,radarr}.json`):

| app | titles | CFs posted | modelable match-parity | score-parity |
|-----|-------:|-----------:|-----------------------:|-------------:|
| Sonarr | 1,165 | 235 | **0.592** | **0.550** |
| Radarr |   544 | 240 | **0.436** | **0.393** |

"Modelable" excludes CFs cellarr can never model (unsupported `implementation`s);
score-parity *before* the fixes below was 0.21 (Sonarr) / 0.11 (Radarr). The
divergence tail is catalogued (unsupported-spec, language-default,
parser-coverage, regex-dialect) in [decision-gaps.md](decision-gaps.md), not
chased.

### The four CF bugs the oracle caught (G-CF1..4)

Each was a silent, high-blast-radius matching bug — verified live against the
apps' `/api/v3/parse` — and each now has a hermetic regression in
`trash_cf_static.rs`:

- **G-CF1 — case-insensitive CF regexes.** cellarr matched CF `ReleaseTitle`
  regexes case-*sensitively*; the apps compile them `IgnoreCase`, and TRaSH CFs
  are written lowercase. First oracle run: **78.3% (94/120)**, every miss on an
  UPPERCASE token (HEVC, REPACK, AMZN…). Fix: `RegexBuilder::case_insensitive(true)`
  → **100%**. Without this cellarr would have matched almost no real-world CF.
- **G-CF2 — `ReleaseGroupSpecification` is a regex, not exact-equality.** The apps
  compile the spec value as a case-insensitive regex on the parsed group; cellarr
  compared strings, so `No-RlsGroup` (negated "has no group") matched nearly
  everything. Fix: compile + evaluate ReleaseGroup as a regex against
  `parsed.group`.
- **G-CF3 — `SourceSpecification` enum indices are app-specific.** Sonarr's and
  Radarr's `QualitySource` enums differ (index `7` = *blurayRaw* on Sonarr,
  *WEB-DL* on Radarr). A single shared mapping mis-matched a whole class of CFs.
  Fix: dialect-specific importer (`import_trash_custom_formats*_for_app`),
  dialect-specific `source_from_index`.
- **G-CF4 — CF boolean algebra is implementation-grouped, not flat-OR.** The apps
  OR non-required conditions *within* an implementation and AND *across*
  implementations; cellarr did a flat OR, so a tier CF (`Source=web` + group
  regexes) matched *every* WEB release. This was the single biggest divergence
  (the Anime/Asian "Tier" clusters). Fix: `MatchContext::matches` groups by
  `discriminant(ConditionKind)`. Verified live with crafted probe CFs.

## 6. Mutation testing — proof the tests aren't fake green

`cargo mutants` was run, scoped per critical source file, against the three
safety-critical crates. Output (`mutants.out/`) is **git-ignored and not
committed** (see `.gitignore`). The survivor-killing tests in `mutation_kills.rs`
were written to fail when a specific operator/condition is flipped, and that was
**verified empirically**: with `cellarr-decide/tests/mutation_kills.rs` present,
the `^ → |` and `^ → &` mutants on `matching.rs:202` (the `raw ^ negate`
inversion) are **caught**; remove that one test file and the `^ → |` mutant
**survives (MISSED)**. The test is load-bearing, not decorative.

Mutation score per critical crate (caught / (caught + missed); unviable mutants
excluded), from each crate's `mutants.out/outcomes.json`:

| crate | caught | missed | score |
|-------|-------:|-------:|------:|
| cellarr-fs | 57 | 0 | **100.0%** |
| cellarr-decide | 63 | 0 | **100.0%** (9 unviable excluded) |
| cellarr-parse | 43 | 11 | **79.6%** |

The 11 `cellarr-parse` survivors are all arithmetic/boundary mutants inside
`numbering.rs` (`extract`, `expand_episode_list`) where flips like `- → +`,
`< → <=`, `/` on internal index math do not change the parsed result on the
covered inputs; they are tracked in `mutants.out/missed.txt`, not hidden.

The `mutation_kills.rs` suites pin: the decide.rs full-season re-grab guard (all
four AND conditions, both directions), the higher-quality upgrade gating
(`upgrades_allowed && rank < cutoff`) and the `>= cutoff` CutoffAlreadyMet
boundary, the `is_proper_or_repack` raw-title `||` fallback, and the `raw ^ negate`
inversion for ReleaseTitle / IndexerFlag / Size conditions — each with a positive
control so it cannot pass vacuously.

## 7. Coverage

From an actual `cargo llvm-cov -p cellarr-parse -p cellarr-decide -p cellarr-fs
--summary-only` run across the three critical crates:

| metric | value |
|--------|------:|
| region coverage | **95.37%** (4,945 / 5,185) |
| line coverage | **96.16%** (3,031 / 3,152) |
| function coverage | 92.80% (438 / 472) |

The decision algebra is essentially fully covered: `decide.rs` 100% lines,
`matching.rs` 100% lines, `scoring.rs`/`quality.rs` 100%. The uncovered tail is
in `cellarr-fs` I/O error branches (`fsops.rs`, `import.rs`) and parser
entry-point glue (`lib.rs`), not in the core logic.

## 8. Property tests + fuzz

All property tests assert real invariants with positive controls (no
`assert!(true)`), over non-trivial generators:

**`proptest_precedence.rs`** (cellarr-decide, 400 cases each):
- adding a positive matching CF never lowers the total score (monotonicity);
- a strictly worse-quality candidate is never an Upgrade even with a huge CF score
  (quality dominates CF) — gated by `prop_assume!(cand.rank < disk.rank)`;
- once both cutoffs are met, an equal candidate is `CutoffAlreadyMet` (no churn);
- the full parse→match→decide path never panics on arbitrary ASCII fragments.

**`proptest_naming.rs`** (cellarr-fs, 600 cases each) over an adversarial
token-value strategy (reserved chars, embedded `/`, unicode, control bytes,
trailing dot/space, `CON`, `AC/DC`, `Quo Vadis: Aida?`):
- `render_name` never panics;
- a successful Windows render carries no reserved char, no control code, no
  trailing dot/space;
- a value's `/` can never create a directory level;
- render is deterministic;
- sanitization is idempotent (an already-sanitized name is a fixed point);
- `render_name` agrees with `render_name_with(default)`.

**`proptest_no_panic.rs`** (cellarr-parse, 2,000 cases): `parse_title` never
panics on arbitrary unicode or release-like token soup, and is deterministic.

**Fuzz target** `parse_title.rs` (libFuzzer, `cargo +nightly fuzz run
parse_title`): interprets raw fuzzer bytes as a lossy-UTF-8 title and asserts the
parser is a total function — it returns a `ParsedRelease` for every input and
never unwinds — plus an inline determinism check (a second parse must be
byte-identical). The corpus under `fuzz/corpus/parse_title/` seeds it; both
`fuzz/target/` and the corpus are git-ignored.

---

## Reproducing the numbers

```sh
just ci                                    # full gate (build, fmt, clippy -D warnings, test)
just test                                  # hermetic workspace tests (529 pass)
just oracle                                # differential parser oracle (Docker)
just oracle-cf  && just oracle-trash-cf    # CF match/score oracles (Docker)
# coverage (3 critical crates):
cargo llvm-cov -p cellarr-parse -p cellarr-decide -p cellarr-fs --summary-only
# mutation (scoped, time-boxed — see the anti-wedge note in the agent docs):
cargo mutants -p cellarr-decide -f src/matching.rs --timeout 60
# fuzz (nightly):
cd crates/cellarr-parse/fuzz && cargo +nightly fuzz run parse_title -- -max_total_time=60
```
