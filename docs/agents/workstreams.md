# Workstreams — the parallelizable work breakdown

This is the fan-out map for swarms of agents. It shows what can be built **in parallel** (because
it sits behind a trait or a fixture set) and what must be **serialized** (because of a real
dependency). Pair this with [../14-roadmap.md](../14-roadmap.md) (phase order) and the per-crate
specs in [../specs/](../specs/).

## How to parallelize safely

The system is designed so agents don't collide:

- **Traits are seams.** `cellarr-core` defines the cross-crate traits (`MediaModule`,
  `MetadataSource`, repository traits, indexer/download-client traits). Once a trait exists, the
  crate *behind* it and the crate *in front* of it can be built simultaneously, each against a mock.
- **Fixtures decouple integrations.** Every indexer/client/metadata adapter is built against
  recorded HTTP fixtures, so adapter agents never wait on each other or on live services.
- **The corpus decouples the domain components.** Parser and decision engine are built against
  `/corpus`, independently.

**Rule:** the *first* thing to land in any new crate is its **trait/interface from the spec plus a
mock and its test scaffold**. After that, work forks.

## Critical path (must be roughly in order)

1. `cellarr-core` traits + shared types  →  unblocks everyone.
2. The corpus + differential oracle  →  unblocks parser & decision validation.
3. `cellarr-db` schema + repository traits  →  unblocks anything that persists.
4. Pipeline state machine in `cellarr-core`  →  unblocks `cellarr-jobs` wiring.
5. `cellarr-fs` import discipline  →  gates anything that touches the library.

Everything else hangs off these and parallelizes widely.

## Parallel workstreams (can run concurrently once their dep above exists)

| Stream | Crate(s) | Depends on | Decoupled by |
|--------|----------|------------|--------------|
| **Parser** | `cellarr-parse` | core types, corpus | `/corpus/parse` + oracle |
| **Decision engine** | `cellarr-decide` | core types, corpus | `/corpus/scoring` + oracle |
| **Persistence** | `cellarr-db` | core traits | repository tests (both engines) |
| **Metadata** | `cellarr-meta` | `MetadataSource` trait | record/replay fixtures |
| **Media modules** | `cellarr-media` | `MediaModule` trait, meta | mocked `MetadataSource` |
| **Indexers** | `cellarr-indexers` | indexer trait | record/replay + caps fixtures |
| **Download clients** | `cellarr-download` | client trait | record/replay (per version) |
| **File ops** | `cellarr-fs` | core types | temp-fs + fault-injection tests |
| **Jobs/scheduler** | `cellarr-jobs` | pipeline SM, db | in-memory job tests |
| **API** | `cellarr-api` | repository traits | mocked repos; `/api/v3` recorded pairs |
| **UI** | `web/` | API shapes (OpenAPI) | mock API server + SRCL |
| **Migration** | `cellarr-migrate` | db schema | sanitized fixture DBs |
| **LLM fallback** | `cellarr-llm` | parser interfaces | local model; cached fixtures |
| **Plugins** | `cellarr-plugins` | host WIT | sample WASM guests |

Within a single integration crate, **each adapter is its own parallel unit** (one agent per
download client, one per metadata source), since each has its own fixture set.

## Sizing tasks for agents

- A good agent task = **one trait impl or one adapter or one extractor**, with its tests, behind a
  stable interface. Small enough to finish and verify; large enough to be a meaningful slice.
- Always include the test obligation in the task. "Implement the qBittorrent adapter" is incomplete;
  "Implement the qBittorrent adapter and its record/replay fixtures incl. the 5.x login variants" is
  a task.
- When two tasks would edit the same file, that's a smell — re-cut them along the trait boundary.

## Coordination rules

- **Specs are shared truth.** If your work reveals a spec is wrong, change the **spec** (and flag
  it) before changing code — others are building against it.
- **Don't widen a public interface to make your crate easier.** Raise it on the spec.
- **Grow the corpus/fixtures** whenever you learn an edge case — that's how the swarm compounds
  knowledge instead of relearning it.
- Anything touching a **non-negotiable** (library safety, offline, single-binary, `/api/v3`,
  SRCL-only, clean-room) is not an autonomous decision — flag it.
