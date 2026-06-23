# Definition of Done

A task is **done** when all of the following hold. "It compiles" and "it looks right" are not on the
list. If you cannot satisfy an item, the task is not done — say so explicitly rather than implying
completion.

## Every task

1. **The behavior is specified as tests, and they pass.** New behavior ships with new tests; fixed
   bugs ship with a regression test (often a new corpus vector or fixture).
2. **The crate's full test suite passes:** `cargo test -p <crate>`.
3. **Formatting & lints clean:** `cargo fmt --check` and
   `cargo clippy --all-targets -- -D warnings` pass for the crate.
4. **No new panics on user-reachable input.** Fallible paths return typed errors.
5. **Docs updated** if behavior described in `docs/` changed (docs are the plan of record).
6. **No non-negotiable violated** (single-binary/offline/library-safety/`api-v3`/SRCL-only/
   clean-room). If the task brushed one, it's explicitly noted and was agreed.
7. **Knowledge captured:** any edge case discovered is encoded as a corpus vector / fixture / doc
   note so the next agent doesn't relearn it.

## Component-specific gates

- **Parser (`cellarr-parse`):** corpus parity meets the current milestone threshold
  ([../14-roadmap.md](../14-roadmap.md)); differential-oracle parity not decreased; `proptest`
  finds no panics; intentional divergences allow-listed with rationale.
- **Decision engine (`cellarr-decide`):** scoring/decision corpus passes; oracle decision parity not
  decreased; precedence rules have dedicated tests.
- **Persistence (`cellarr-db`):** repository tests pass on **both** SQLite and Postgres; migrations
  apply from empty on both; offline `.sqlx` committed and current.
- **File ops (`cellarr-fs`):** crash-safety tests pass (injected failure between
  stage/verify/commit/cleanup leaves a consistent, resumable state); cross-fs move never leaves a
  partial destination; old file never removed before new file durable.
- **Integrations (`cellarr-indexers`, `cellarr-download`, `cellarr-meta`):** record/replay fixtures
  cover the happy path **and** known version/quirk divergences; no live service on the CI path.
- **API (`cellarr-api`):** handler tests pass; `/api/v3` contract tests pass against recorded pairs;
  a WS push test passes.
- **UI (`web/`):** `tsc --noEmit` clean; component tests pass; the **SRCL-only lint** passes (no UI
  primitive outside the SRCL set).
- **Migration (`cellarr-migrate`):** fixture-DB import mappings asserted; "recognize in place"
  (no file ops on already-correct files) verified.

## The full CI gate (what runs on a PR)

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace                 # unit + corpus + repository + record/replay + integration
<differential oracle>                  # parity per component ≥ milestone threshold
<repository tests on Postgres>         # service-container job
web: tsc --noEmit && vitest run && <srcl-only lint>
<sqlx offline metadata check>          # .sqlx up to date
```

## Verification, not assumption

For anything observable, **verify behavior**, not just that code compiles or that a unit test is
green in isolation. If a change affects the pipeline, run the relevant integration slice. If it
affects the UI, render the screen against the mock API. Report what you actually observed — if a
step was skipped or a test is flaky, say so plainly.
