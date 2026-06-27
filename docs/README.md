# cellarr documentation

This directory **is** the project right now. It is the end-to-end plan: enough for swarms of
agents to build and test cellarr without further design decisions from a human.

## Reading order

Read top to bottom the first time. After that, jump to the doc you need.

| # | Doc | What it covers |
|---|-----|----------------|
| — | [agents/AGENTS.md](agents/AGENTS.md) | **Start here if you are an agent.** How to work in this repo. |
| 00 | [00-vision.md](00-vision.md) | What cellarr is, scope, the four pillars, non-negotiables. |
| 01 | [01-architecture.md](01-architecture.md) | System architecture, crate workspace, runtime, concurrency. |
| 02 | [02-data-model.md](02-data-model.md) | The unified media data model (generic structure, typed identity). |
| 03 | [03-pipeline.md](03-pipeline.md) | The acquisition pipeline state machine and the decision log. |
| 04 | [04-parser.md](04-parser.md) | The release-name parser and the corpus strategy. |
| 05 | [05-decision-engine.md](05-decision-engine.md) | Quality profiles + custom-format scoring. |
| 06 | [06-integrations.md](06-integrations.md) | Indexers, download clients, and the WASM plugin host. |
| 07 | [07-metadata-service.md](07-metadata-service.md) | The self-hostable metadata service (Skyhook rebuild). |
| 08 | [08-database.md](08-database.md) | SQLite/Postgres, the writer-actor, schema/migrations. |
| 09 | [09-api.md](09-api.md) | REST + WebSocket API and the *arr v3 compatibility shim. |
| 10 | [10-ui.md](10-ui.md) | The frontend, built exclusively from Sacred/SRCL components. |
| 11 | [11-testing.md](11-testing.md) | Test strategy: corpus, differential oracle, levels, CI gates. |
| 12 | [12-migration.md](12-migration.md) | Importing from existing Radarr/Sonarr/Lidarr installs. |
| 13 | [13-upstream-repos.md](13-upstream-repos.md) | Every upstream source: what to learn, what to reuse, licensing. |
| 14 | [14-roadmap.md](14-roadmap.md) | Phased build order and milestones. |
| 15 | [15-glossary.md](15-glossary.md) | Domain glossary — read when a term is unfamiliar. |
| 16 | [16-local-dev-and-testing.md](16-local-dev-and-testing.md) | Local dev/test + parallel-workflow isolation (ports, DBs, Docker, worktrees). |
| 17 | [17-config-as-code.md](17-config-as-code.md) | Declarative managed config: the YAML file, `${ENV}` secrets, safe prune, the `managed-config` CLI. |

## Specs (the contracts)

Each crate has a spec in [`specs/`](specs/) defining its responsibility, its public interface,
its dependencies, and — most importantly — its **test obligations**. Agents implement *against
the spec and its tests*, not against each other's code.

## Agent operating docs

- [agents/AGENTS.md](agents/AGENTS.md) — the operating model and rules.
- [agents/workstreams.md](agents/workstreams.md) — the parallelizable work breakdown for swarms.
- [agents/conventions.md](agents/conventions.md) — code, comment, commit, and test conventions.
- [agents/definition-of-done.md](agents/definition-of-done.md) — when a task is actually finished.
- [agents/legal-and-licensing.md](agents/legal-and-licensing.md) — clean-room rules for reusing upstream material.

## How this plan was built

The architecture was designed from first principles; the upstream facts (file paths, fixture
formats, protocol details, crate choices, licensing) were researched against the live upstream
repositories and recorded in [13-upstream-repos.md](13-upstream-repos.md). Where a fact is
uncertain, the doc says so explicitly — verify before relying on it.
