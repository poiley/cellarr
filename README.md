# cellarr

**A unified, self-hosted media library manager — one daemon for movies, TV, music, and books — written in Rust.**

cellarr is a greenfield reimplementation of the *arr stack (Radarr, Sonarr, Lidarr, Readarr,
Prowlarr) as a *single* application with a clean architecture, a fast and safe core, and a
modern terminal-aesthetic UI. It learns from a decade of accumulated domain knowledge in the
existing apps — without inheriting their fork-sprawl, their polling-heavy design, or their
two-codebases-that-drift problem.

> **Status:** v1 build complete and green. The full stack is implemented and tested — the
> daemon boots zero-config, runs the acquisition pipeline, serves the native + `/api/v3` APIs,
> and serves the theme-aware SRCL web UI from one binary. `just ci` passes (cargo 303 tests +
> web 92 tests, clippy + fmt + SRCL-only lint clean). See [`BUILD_STATUS.md`](BUILD_STATUS.md)
> for exactly what's built vs. deferred. Start at [`docs/README.md`](docs/README.md) for the design.

---

## Why this exists

The existing *arr apps are excellent and battle-tested, but their architecture shows its
NzbDrone-fork lineage:

- **Five near-identical codebases** (Radarr is a fork of Sonarr; Lidarr/Readarr/Prowlarr share
  the same `NzbDrone.Core` base) that re-port fixes to each other forever.
- **Polling-heavy** RSS sync and status checks rather than event-driven flows.
- **Separate apps** (Radarr + Sonarr + Prowlarr) that the user must wire together.
- A frontend and API that grew organically over years.

The *value* of the *arr stack is **not** its architecture — it is the accumulated, adversarial
domain knowledge encoded in its release-name parser, its import/upgrade decision logic, and its
integration long tail. cellarr's thesis is: **keep the hard-won domain knowledge (as test
corpora and behavioral specs), throw away the architecture, and unify everything into one
correct, fast, observable daemon.**

## The four pillars

1. **One daemon, all media types.** Movies, TV, music, and books are *modules* behind a trait,
   not separate apps. Add a media type by implementing one interface, not forking the engine.
2. **Stand on the shoulders of giants.** We mine the existing apps' **test fixtures** as a
   language-neutral corpus, treat their behavior as an executable spec, and reuse community data
   (Cardigann indexer definitions, TRaSH custom-format scores, XEM/anime-list mappings). See
   [`docs/13-upstream-repos.md`](docs/13-upstream-repos.md) for the what/where/license of every
   source — and the clean-room rules in [`docs/agents/legal-and-licensing.md`](docs/agents/legal-and-licensing.md).
3. **Tests are the contract.** Every component is built against a corpus of real inputs and a
   *differential oracle* that diffs cellarr's output against the original apps. "Are we correct
   yet?" is a number we watch climb in CI. See [`docs/11-testing.md`](docs/11-testing.md).
4. **Terminal-aesthetic UI, exclusively from Sacred (SRCL).** Every screen is composed *only*
   from components in [`internet-development/www-sacred`](https://github.com/internet-development/www-sacred)
   (the `srcl` library at [sacred.computer](https://www.sacred.computer/)). No bespoke UI
   primitives. See [`docs/10-ui.md`](docs/10-ui.md).

## Non-negotiables

These constrain every decision. An agent proposing anything that violates one of these is wrong:

- **One static binary, one container, zero required external services.** SQLite + embedded
  everything by default. No mandatory Redis/Postgres/Elasticsearch.
- **Works fully offline** except for the network calls inherent to the job (indexers,
  metadata, download clients). No feature may *require* a cloud LLM or paid SaaS to function.
- **Never corrupt the user's library.** Imports move and delete files. Every destructive
  operation is staged, verified, committed, and logged so a crash never leaves the library
  half-written, and so the user can always answer "why did it do that?"
- **Ecosystem compatible.** Emulate the Radarr/Sonarr v3 REST API so existing tools
  (Overseerr/Jellyseerr, Notifiarr, etc.) work without modification.
- **UI is exclusively Sacred/SRCL components.**

## Repository layout (planned)

```
cellarr/
├── docs/                  # THE PLAN (this is what exists today) — read docs/README.md first
│   ├── specs/             # per-crate contracts agents implement against
│   └── agents/            # how agent swarms operate in this repo
├── corpus/                # language-neutral test vectors (parser, scoring, naming, …)
├── reference/             # cloned upstream repos for study (git-ignored, never shipped)
│   └── www-sacred/        # the SRCL UI library (already cloned)
├── crates/                # Rust workspace (not yet created)
└── web/                   # Next.js frontend built exclusively from SRCL (not yet created)
```

## For agents

If you are an agent picking up work here, read **in this order**:

1. [`docs/agents/AGENTS.md`](docs/agents/AGENTS.md) — how to work in this repo, the rules, the loop.
2. [`docs/README.md`](docs/README.md) — the documentation index and reading order.
3. The spec for the crate you are assigned in [`docs/specs/`](docs/specs/).
4. [`docs/11-testing.md`](docs/11-testing.md) — because nothing merges without tests.

## License

To be decided before first code lands; the choice is **load-bearing** and interacts with how we
reuse upstream material. See [`docs/agents/legal-and-licensing.md`](docs/agents/legal-and-licensing.md).
Until decided, treat the project as if it will be **GPLv3** (the safe assumption given our
sources) and follow the clean-room rules.
