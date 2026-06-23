# 16 — Local development & testing (and parallel-workflow isolation)

This machine runs **many workflows at once**, each building and testing its own version of cellarr.
This doc defines how that works without collisions: ports, databases, Docker containers, build
artifacts, and scratch dirs are all **namespaced per run** and use **OS-allocated ports**. Nothing
in the test/dev flow may assume it is the only one running.

> **Reminder:** cellarr is private. Local commits are fine; **never push** (see [../CLAUDE.md](../CLAUDE.md)
> — enforced by `.githooks/pre-push`).

## TL;DR

```sh
just setup      # one-time: mise toolchains, wire git hook, web deps
just test       # fast hermetic suite (SQLite + record/replay; no Docker, no ports)
just test-pg    # repository tests vs an ephemeral, per-run Postgres
just oracle     # parity vs pinned Sonarr/Radarr (per-run compose, ephemeral ports)
just web-test   # typecheck + vitest + SRCL-only lint
just ci         # the full gate
just ports      # show THIS run's port block
just clean-run  # tear down this run's containers/DBs/scratch
```

## Toolchain

Rust/Cargo and Node are **not** installed globally on this machine; they are pinned in
[`.mise.toml`](../.mise.toml) and installed by `just setup` (`mise install`). Always use the
mise-managed toolchain. The build needs Docker (OrbStack is present) only for `test-pg` and `oracle`;
`just test` and `just web-test` need neither Docker nor network.

## The isolation model

### RUN_ID — the namespace key
Every run has a `RUN_ID`. By default it is the **git worktree's folder name** (sanitized);
override with the `CELLARR_RUN_ID` env var. Everything ephemeral is keyed off it.

### Use a git worktree per parallel workflow
The cleanest isolation is one **git worktree** per concurrent piece of work. Each worktree gets:
- its own source checkout and branch,
- its own `target/` (cargo's default is per-directory — do **not** set a shared `CARGO_TARGET_DIR`),
- its own `RUN_ID` (the folder name) → its own port block, container names, and Postgres DBs,
- its own gitignored `./.run/<RUN_ID>/` data dir for manual `just dev`.

Two worktrees therefore never collide. When launching agents for parallel work, prefer
`isolation: "worktree"`. (cargo holds a per-target build lock, so even if two runs *shared* a
target they would serialize rather than corrupt — but per-worktree targets keep them fully parallel.)

### Ports: a deterministic per-run block, OS-allocated elsewhere
- **Long-lived dev servers** (`just dev`: api/meta/web) use a **port block** derived by hashing
  `RUN_ID` into `[20000, 59990]` stepped by 10 (`just ports` prints it). Stable for a given worktree,
  distinct across worktrees. Collision probability across a handful of concurrent runs is negligible;
  if two ever clash, rename the worktree or set `CELLARR_RUN_ID`.
- **Tests never hardcode a port.** Servers under test bind `127.0.0.1:0` and the test reads back the
  actual port. This is a hard rule (enforced in review): a test that binds a fixed port is a bug.
- **Docker-mapped services** (Postgres, oracle) publish to host port `0` (OS-allocated) and the
  recipe discovers the real port via `docker port` / `docker compose port`. No fixed host ports.

### Filesystem: tempdirs and per-run scratch
- **Tests** write only to per-test temp directories (`tempfile` crate) — never a fixed path. This is
  what makes `just test` safe to run in N worktrees at once. `cellarr-fs` crash-safety tests use
  temp filesystems ([specs/cellarr-fs.md](specs/cellarr-fs.md)).
- **`just dev`** puts the app's data dir (SQLite file, downloads workspace) under `./.run/<RUN_ID>/`,
  which is gitignored. `just clean-run` removes it.

### Databases
- **SQLite (default):** file-based; tests use a fresh temp file per test → naturally isolated.
- **Postgres (`just test-pg`):** spins an **ephemeral container per run** named `cellarr-pg-<RUN_ID>`
  on an ephemeral host port, waits for readiness, exports `CELLARR_TEST_DATABASE_URL`, runs the
  repository suite, and tears it down on exit. Many runs coexist because both the container name and
  the host port are unique. (A faster alternative for heavy use — one shared PG instance with a
  unique database `cellarr_test_<RUN_ID>` per run — is acceptable; default to the ephemeral container
  for clean isolation.)

### The differential oracle
`just oracle` brings up **pinned** Sonarr + Radarr via [`tests/oracle/docker-compose.yml`](../tests/oracle/docker-compose.yml)
under a **per-run compose project** (`-p cellarr-oracle-<RUN_ID>`), which namespaces all containers,
networks, and volumes for that run. Ports are ephemeral and discovered at runtime; the harness reads
`CELLARR_ORACLE_SONARR` / `CELLARR_ORACLE_RADARR`. The stack is torn down (`down -v`) on exit. Image
pins are exact and deliberate — bumping one changes the parity baseline and is a reviewed change
([11-testing.md](11-testing.md)).

## What this buys the swarm

- Any number of agents can run `just test` / `just test-pg` / `just oracle` **simultaneously** from
  their own worktrees with zero coordination.
- A run's footprint is fully discoverable (`just ports`) and fully removable (`just clean-run`).
- CI uses the exact same recipes, so "passes locally" and "passes in CI" mean the same thing.

## Rules for test authors (enforced in review)

1. **No fixed ports.** Bind `:0`; read the assigned port. Same for any spawned subprocess server.
2. **No fixed paths.** Use tempdirs; clean them up. Never write under the repo except `./.run/<id>/`.
3. **No shared mutable services.** If you need Postgres/Docker, namespace it by `RUN_ID` via the
   Justfile recipes; don't reach for a hand-started global instance.
4. **No live third parties on the critical path.** Use record/replay fixtures
   ([11-testing.md](11-testing.md)); live suites are opt-in and off the default path.
5. **Leave nothing running.** Recipes trap-cleanup their containers; ad-hoc test code must too.
6. **Concurrency courtesy.** The oracle and `test-pg` start real containers — don't fan out dozens at
   once. Keep heavy Docker suites to a sensible parallelism on a dev machine.

## CI parity

`just ci` runs the same gates a PR must pass (lint, hermetic tests, Postgres repository tests,
web checks). The Postgres and oracle jobs in CI use the identical per-run namespacing, so a green
local `just ci` predicts a green pipeline. The authoritative gate list is in
[agents/definition-of-done.md](agents/definition-of-done.md).
