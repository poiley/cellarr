# 11 — Testing strategy

Tests are how cellarr is correct, and how a swarm of agents stays coordinated without reading each
other's code. This doc defines the levels, the corpus, the differential oracle, and the CI gates.
**Nothing merges without the tests its spec requires.**

## Principles

1. **Tests are the contract.** A spec defines required behavior as tests. You implement until they
   pass. You do not change a test to make code pass unless the test was wrong (and that's a flagged
   change).
2. **Correctness is a number.** For the domain-knowledge components (parser, decision engine), we
   measure parity against the originals and watch it climb. "Looks right" is not a status.
3. **No live third parties on the critical path.** Indexers, download clients, and metadata sources
   are tested via recorded fixtures. Live tests exist but are opt-in and off the critical path.
4. **The dangerous code gets the most tests.** File operations (import/rename/delete) and the
   writer-actor get crash-safety and property tests, because they can destroy user data.

## The test levels

| Level | What | Where | Speed |
|-------|------|-------|-------|
| **Unit** | pure functions, extractors, scoring | in each crate | instant |
| **Corpus** | table-driven vectors (parse, score, naming) | `/corpus` + crate harness | fast |
| **Repository** | DB repos against SQLite **and** Postgres | `cellarr-db` | fast |
| **Record/replay** | integrations & metadata vs recorded HTTP | per integration crate | fast |
| **Differential oracle** | cellarr vs real *arr apps | dedicated harness | medium (Docker) |
| **Integration** | pipeline slices wired together | workspace tests | medium |
| **End-to-end** | API + UI + a fake indexer/client | `web/` + harness | slower |
| **Live smoke** (opt-in) | real indexers/clients/sources | tagged, manual/nightly | slow, flaky-tolerant |

## The corpus (`/corpus`)

Language-neutral test vectors (TOML/JSON), the executable form of the domain knowledge we mine from
upstream. Layout and contents are defined per consumer:

- `corpus/parse/*` — release-name → expected fields (see [04-parser.md](04-parser.md)).
- `corpus/scoring/*` — CF match / score / decision vectors (see [05-decision-engine.md](05-decision-engine.md)).
- `corpus/naming/*` — content + metadata → expected on-disk path (rename engine).
- `corpus/anime/*` — absolute ↔ season/episode mapping expectations (shared parser + metadata).

Each vector records its **provenance** (`source`) and optional `notes`. Vectors are **re-curated**,
not copied verbatim from upstream fixture files (see [agents/legal-and-licensing.md](agents/legal-and-licensing.md)).
Building the corpus *precedes* building the component it tests.

## The differential oracle

For the components where the originals are the de-facto spec (parser, decision engine, and parts of
naming/identify), a harness:

1. Runs **real Sonarr/Radarr** (pinned versions, in Docker) and drives their parser/decision
   surface (their test endpoints / API) over a large input set.
2. Runs cellarr over the same inputs with equivalent config.
3. **Diffs** the outputs and reports a **parity percentage** per category.

Rules:
- Parity is a CI metric tracked over time; a merge may not *decrease* parity below the agreed
  threshold for that component (thresholds in [14-roadmap.md](14-roadmap.md) per milestone).
- Where cellarr **intentionally** differs (a known upstream bug we don't replicate, an improvement),
  the case is added to an **explicit allow-list** with a written rationale. Silent divergence is a
  test failure, not a pass.
- The oracle pins exact upstream versions so results are reproducible; bumping the pin is a
  deliberate change.

This is what lets us "copy then improve": the oracle is the safety net that makes refactoring the
parser/decision engine fearless, and the thing that turns "are we as good as Sonarr yet?" into a
dashboard.

## Crash-safety & property tests (file ops and DB)

- **File operations** ([`specs/cellarr-fs.md`](specs/cellarr-fs.md)): tests that inject failures
  between stage/verify/commit/cleanup and assert the library is always consistent and the operation
  is resumable; cross-filesystem move never leaves a partial destination; old file is never removed
  before the new file is durable. Use temp filesystems; consider fault-injection wrappers.
- **Writer-actor / transactions** ([08-database.md](08-database.md)): kill mid-write, reopen, assert
  consistency.
- Property-based tests (`proptest`) for the parser (round-trips, no panics on arbitrary input) and
  the scoring function (monotonicity invariants).

## Record/replay for integrations & metadata

Every indexer/Cardigann/download-client/metadata adapter ships a fixture set of recorded HTTP
exchanges (including `t=caps` and each known-divergent client version, e.g. qBittorrent 5.x). Tests
assert parsing/normalization/lifecycle against fixtures, fully offline. A nightly **drift** suite
hits real services to detect when a recorded shape has gone stale.

## CI gates (every PR)

1. `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` (all crates).
2. Corpus suites pass; component parity ≥ its milestone threshold (oracle, where applicable).
3. Repository tests pass on SQLite and Postgres.
4. Record/replay integration tests pass.
5. `web/`: typecheck + component tests + the **"SRCL-only" UI lint** ([10-ui.md](10-ui.md)).
6. Offline `sqlx` query metadata is up to date (`.sqlx` committed).

The exact commands live in [agents/definition-of-done.md](agents/definition-of-done.md).

## A note for agents writing tests

When you discover a real edge case (a release name the parser botched, an indexer quirk, a client
version difference), **add a vector/fixture for it** in the same change. The corpus and fixture sets
are how the next agent avoids re-learning what you just learned. Growing them is part of the job,
not overhead.
