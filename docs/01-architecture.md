# 01 — Architecture

## Shape: a modular monolith

cellarr is **one binary** (plus an optional second binary for the metadata service that can also
run embedded). Microservices are the wrong answer for a self-hosted app — users want one process
and one container. We get clean seams *and* the single binary by enforcing **hard crate
boundaries** within a Cargo workspace: crates communicate only through traits and typed messages,
never through shared mutable globals.

This gives the swarm what it needs: an agent can build and test `cellarr-parse` against a trait
mock while another agent builds `cellarr-indexers` behind that trait.

## Crate workspace

```
crates/
  cellarr-core       Domain types, the pipeline state machine, the cross-crate traits.
  cellarr-db         Persistence: sqlx repositories, migrations, the writer-actor, SQLite+Postgres.
  cellarr-parse      Release-name parser + the corpus test harness.
  cellarr-decide     Quality profiles, custom formats, release scoring & the decision function.
  cellarr-media      Per-media-type modules (movie/tv/music/book) behind the MediaModule trait.
  cellarr-indexers   Torznab/Newznab clients + the Cardigann YAML definition engine.
  cellarr-download   Download client adapters (qBittorrent, Deluge, Transmission, SAB, NZBGet).
  cellarr-fs         Library file operations: scan, hardlink/copy, atomic move, rename engine.
  cellarr-jobs       Scheduler: cron + on-demand jobs, retry/backoff, per-resource rate limits.
  cellarr-plugins    wasmtime host for WASM Component Model plugins (post-v1).
  cellarr-meta       Metadata normalization + caching (the Skyhook rebuild); usable embedded.
  cellarr-llm        Inference fallback for parsing/matching (local-first, optional).
  cellarr-api        axum REST + WebSocket server, OpenAPI, and the /api/v3 compat shim.
  cellarr-migrate    Importers that read existing *arr SQLite databases.
  cellarr-cli        The binary: wires everything, parses args/config, runs the daemon.
meta-service/        Thin binary wrapping cellarr-meta to run it standalone (or it runs embedded).
web/                 Next.js frontend built exclusively from SRCL (see 10-ui.md).
```

**Dependency direction:** `cellarr-core` depends on nothing internal (it holds the shared types
and trait definitions). Everything depends on `cellarr-core`. `cellarr-cli` depends on everything
and is the only place wiring happens. No crate may depend on `cellarr-cli`. No cycles. The
per-crate specs in [`specs/`](specs/) state each crate's exact allowed dependencies.

## Runtime & concurrency model

The workload is ~95% **I/O-bound** (HTTP to indexers, download clients, and metadata sources)
with **bursts of CPU** (parsing thousands of candidate release titles per search; hashing files
on import). The model follows from that:

- **`tokio`** drives all I/O. Indexer fan-out uses `FuturesUnordered`; nothing blocks the reactor.
- **Per-host rate limiting** with `governor` behind a `tower` layer. Hammering an indexer gets you
  banned — this is a top operational failure in the originals. Rate limits are *per host/indexer*,
  configurable, and conservative by default. AniDB and MusicBrainz have especially strict limits
  (see [07-metadata-service.md](07-metadata-service.md)).
- **CPU bursts → `rayon`.** Parsing N candidate titles is embarrassingly parallel. Regexes are
  compiled once into a `OnceCell`; the multi-pattern "which of these N quality patterns match"
  phase uses `regex::RegexSet` for a single linear pass.
- **File hashing/IO** runs on `spawn_blocking` or rayon, never on the async reactor.
- **One shared pooled HTTP client** (`reqwest`, HTTP/2, retry/backoff via `tower`).

### The writer-actor (critical for SQLite)

SQLite permits exactly one writer at a time. Rather than scatter `SQLITE_BUSY` handling
everywhere, **all writes funnel through a single writer task** behind a bounded `mpsc` channel;
reads use a connection pool. This makes write serialization explicit and removes a whole class of
race bugs. Details and the Postgres alternative are in [08-database.md](08-database.md).

## The pipeline as the backbone

The heart of cellarr is one **state machine** (in `cellarr-core`, executed by `cellarr-jobs`):

```
Discover → Parse → Identify → Decide → Grab → Track → Import → Rename → Notify
```

It is **media-type-agnostic**: it carries a `ContentRef` and never knows what a "season" is. The
`MediaModule` trait supplies type-specific behavior (search terms, matching, naming). Every state
transition appends to the **decision log** so the system can always answer "why did it do that?".
Full detail in [03-pipeline.md](03-pipeline.md).

## Extensibility model

Rust has no stable dynamic-plugin ABI, so we use three tiers, in order of preference:

1. **Compiled-in** for the core integrations (the big download clients, native Torznab/Newznab).
   Fastest, type-safe, fully tested.
2. **Declarative data** for indexers: a **Cardigann YAML engine** that interprets existing
   community indexer definitions at runtime. Hundreds of indexers become a folder of data, not
   code. See [06-integrations.md](06-integrations.md).
3. **WASM Component Model** (`wasmtime`, WIT-defined interfaces) for third-party custom
   integrations, notifiers, and metadata sources — sandboxed, language-agnostic, hot-loadable.
   Post-v1. See [06-integrations.md](06-integrations.md) and [`specs/cellarr-plugins.md`](specs/cellarr-plugins.md).

## Observability

- **`tracing`** everywhere with structured spans; the **decision log** (a database table) is the
  domain-level audit trail and the answer to user "why?" questions.
- Optional Prometheus metrics endpoint and OTLP export via `tracing-opentelemetry`. Never required.

## Configuration

Layered config (`figment`): built-in defaults → config file → environment variables. The app must
run with **zero configuration** out of the box (sensible defaults, SQLite in the data dir). Secrets
(API keys, download-client passwords) are stored in the database, encrypted at rest; never logged.

## Why these choices (rationale & alternatives)

- **Rust** for a safe, fast, single-binary daemon with fearless concurrency and excellent file-IO
  control. The release-name parser benefits specifically from Rust's linear-time `regex` engine,
  which cannot catastrophically backtrack on adversarial release titles (a real risk with the
  .NET regex dialect). See the regex caveat in [04-parser.md](04-parser.md).
- **Modular monolith** over microservices: matches the self-host deployment model while keeping
  clean internal boundaries.
- **Trait-bounded crate seams** so the swarm can parallelize without stepping on each other.

The concrete crate/library picks (axum, sqlx, reqwest, rayon, governor, moka, wasmtime, tracing,
serde, figment) and their rationale and alternatives are catalogued in
[13-upstream-repos.md](13-upstream-repos.md) §crates so they live in exactly one place.
