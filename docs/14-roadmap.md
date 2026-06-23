# 14 — Roadmap & build order

The order is chosen so the **risky, knowledge-dense work is validated first**, and so each phase
produces something testable. Phases are sequenced by dependency, not calendar. Each phase lists its
**exit criteria** (what must be true to move on) and the **parity thresholds** the differential
oracle must hit ([11-testing.md](11-testing.md)).

The work *within* a phase is heavily parallelizable across agents — see
[agents/workstreams.md](agents/workstreams.md) for the fan-out.

## Phase 0 — Foundations & corpus
Set up the workspace skeleton, CI, conventions, and **build the corpus and the differential oracle
before feature code**. The corpus is the spec; the oracle is the safety net.
- Cargo workspace with all crate stubs + dependency graph enforced.
- `cellarr-core` skeleton: shared types (`ContentRef`, `Coordinates`, `ParsedRelease`), the
  cross-crate traits (`MediaModule`, `MetadataSource`, repository traits).
- `/corpus` populated from upstream fixtures (parse first), with provenance.
- Differential-oracle harness running pinned Sonarr/Radarr in Docker, diffing against a stub.
- CI gates wired ([definition-of-done.md](agents/definition-of-done.md)), including the SRCL-only UI lint.
- **Exit:** corpus loads and runs; oracle produces a (near-zero) parity number against the stub; CI green.

## Phase 1 — The parser
Build `cellarr-parse` to high parity. This is the highest-value, highest-risk component; doing it
first de-risks everything.
- Extractor pipeline; clean-room implementation against `corpus/parse/`.
- Anime absolute-number extraction (mapping comes in Phase 3 with metadata).
- **Exit:** parser parity ≥ **95%** on the static corpus and ≥ **90%** on the oracle's broader set;
  no panics under `proptest`; intentional divergences allow-listed with rationale.

## Phase 2 — Persistence & decision engine
- `cellarr-db`: schema/migrations, the writer-actor, repository traits, SQLite + Postgres in CI.
- `cellarr-decide`: quality profiles + custom formats + the decision function, against
  `corpus/scoring/`; TRaSH import.
- **Exit:** repository tests green on both engines; decision parity ≥ **95%** on the oracle;
  precedence rules (quality-over-score, both-cutoffs) covered by dedicated tests.

## Phase 3 — Metadata & identify
- `cellarr-meta`: TMDb + TheTVDB adapters, caching, record/replay; scene-mapping (TheXEM +
  anime-lists) for anime numbering.
- `cellarr-media`: movie + TV `MediaModule`s (search terms, match, naming).
- **Exit:** identify resolves the anime corpus correctly; record/replay metadata tests green;
  metadata service runs embedded and standalone.

## Phase 4 — Integrations
- `cellarr-indexers`: Torznab/Newznab + the Cardigann engine (record/replay corpus).
- `cellarr-download`: qBittorrent, Deluge, Transmission, SABnzbd, NZBGet (record/replay, incl.
  version-divergent qBittorrent fixtures).
- `cellarr-jobs`: scheduler, retries, per-host rate limits.
- **Exit:** a release can be discovered, parsed, decided, grabbed, and tracked to completion against
  fake/recorded services end to end.

## Phase 5 — Import, the pipeline, and safety
- `cellarr-fs`: scan, hardlink/copy, atomic move, rename engine — with the stage→verify→commit→log
  discipline and crash-safety tests.
- Wire the full pipeline state machine ([03-pipeline.md](03-pipeline.md)) with the decision log.
- **Exit:** end-to-end movie+TV acquisition against fakes, with crash-safety property tests passing;
  the library is never left inconsistent under injected failures.

## Phase 6 — API, UI, migration
- `cellarr-api`: native REST/WS + the `/api/v3` compatibility shim (contract-tested).
- `web/`: the v1 screens, **exclusively SRCL** ([10-ui.md](10-ui.md)); the decision-log screen is a
  signature feature.
- `cellarr-migrate`: import from real Radarr/Sonarr SQLite DBs.
- **Exit:** Overseerr/Jellyseerr works against `/api/v3`; a user can migrate an existing install and
  browse/search/grab from the UI.

## Phase 7 — Hardening & "feature complete"
- Performance passes (parse throughput, large-library queries), observability polish, packaging
  (single binary + container), docs for end users.
- **Exit (feature complete, per [00-vision.md](00-vision.md)):** a Radarr+Sonarr+Prowlarr user
  migrates with **no regression** in parsing accuracy, decision quality, or integration coverage,
  and gains the unified app + decision log + faster core. Oracle parity ≥ **99%** (movies/TV) with
  all divergences deliberate and documented.

## Post-v1 (designed-for, built later)
- Music (MusicBrainz) and Books (OpenLibrary) `MediaModule`s — slot into the existing model.
- WASM plugin host (`cellarr-plugins`) GA.
- LLM parser fallback (`cellarr-llm`) tuned and promoted from experimental.
- Optional TUI using SRCL's Simulacrum CLI framework.

## Tracking parity
The oracle parity numbers per component are the project's primary health metric. They live in CI and
should be visible on a dashboard. A phase is not "done" because the code exists — it is done when its
exit criteria, including the parity threshold, are met.
