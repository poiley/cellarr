# CLAUDE.md — cellarr project rules

Project-level instructions for any agent working in `~/repos/cellarr`. These override defaults and
sit alongside the global `~/.claude/CLAUDE.md` (system tools) and the plan in
[`docs/`](docs/README.md). Read [`docs/agents/AGENTS.md`](docs/agents/AGENTS.md) before working.

## Repository

Public personal project: [github.com/poiley/cellarr](https://github.com/poiley/cellarr). Branch,
commit, and push freely. Default branch is `main`. This repo uses a local git identity
(`benjpoile@gmail.com`); other repos use the machine default.

## Non-negotiables (from the plan — memorize)

See [`docs/agents/AGENTS.md`](docs/agents/AGENTS.md) for the full list. In short: one static binary /
zero required services (SQLite default); works offline; never corrupt the user's library
(stage→verify→commit→log); keep `/api/v3` compatible; UI is **exclusively** Sacred/SRCL; learn
upstream behavior clean-room, never transcribe code ([`docs/agents/legal-and-licensing.md`](docs/agents/legal-and-licensing.md)).

## Toolchain

Managed by **mise** (see `.mise.toml`): Rust (stable) and Node. Run `just setup` once to install
toolchains, wire the git hook, and install web deps. Rust/Cargo are **not** globally installed on
this machine — use the project's mise-managed toolchain, not a system one.

**Build/test speed.** `just setup` also wires three accelerators; don't undo them:
- **sccache** (`RUSTC_WRAPPER` in `.mise.toml`) — a shared compile cache. `target/` stays
  per-worktree (isolation intact); only compiled artifacts are shared, so a fresh worktree/agent
  skips recompiling unchanged deps. First build of a given dep set is a full miss that warms the
  cache; the win lands on the *next* cold build. Check it with `sccache --show-stats`.
- **dep debuginfo stripped** (`[profile.dev.package."*"] debug = false` in `Cargo.toml`) — our own
  code keeps line-tables for backtraces; the 500+ third-party crates compile with none, cutting
  link time and `target/` size. Don't widen debuginfo back onto deps to "fix" a debugging session.
- **cargo-nextest** powers `just test` (faster, better-parallelized). It does **not** run doctests,
  so the recipe runs `cargo test --doc` separately — keep both legs if you edit it. nextest is
  installed into `~/.cargo/bin` from its official tarball (its dual-tag GitHub releases don't
  resolve through mise backends); `just test` falls back to plain `cargo test` if it's absent.

## Local development & testing

**Every workflow runs isolated.** Multiple agents test their own versions on this machine at once, so
all local test/dev tooling is namespaced per run and uses OS-allocated ports — no fixed ports, no
shared databases, no fixed paths. The full story is in
[`docs/16-local-dev-and-testing.md`](docs/16-local-dev-and-testing.md). Quickstart:

```sh
just setup            # one-time: toolchains, git hook, web deps
just test             # fast, hermetic: workspace tests (SQLite, no Docker)
just test-pg          # repository tests against an ephemeral, per-run Postgres DB
just oracle           # differential oracle (pinned Sonarr/Radarr in Docker, per-run, ephemeral ports)
just web-test         # typecheck + vitest + the SRCL-only lint
just ci               # the full gate (what a PR must pass)
just ports            # show the ports allocated to THIS run/worktree
just clean-run        # tear down this run's containers, DBs, and ./.run/<id>
```

**Run parallel work in git worktrees.** Each worktree gets its own `RUN_ID` (derived from its
folder name), its own `target/`, its own `./.run/<id>/` data dir, its own port block, and uniquely
named Docker containers / Postgres databases. Two worktrees never collide. When launching agents,
prefer `isolation: "worktree"`.

This is not just for ports/DBs — cargo takes a **per-directory lock on `target/`**, so two builds
in the *same* checkout serialize (you'll see `Blocking waiting for file lock on build directory`,
and a `just test` can fail if it collides with another build's lock). Separate worktrees have
separate `target/` dirs and build in parallel; sccache still shares the compiled-artifact cache
across them. So: never run concurrent agents in one checkout — give each its own worktree.

**New-worktree onboarding: `mise trust` first.** A fresh worktree's `.mise.toml` is untrusted, and
*every* `just` recipe shells out through the mise shims — so all of them fail instantly (not just
`setup`) with `Config files in <path>/.mise.toml are not trusted` until you run
`mise trust <worktree>/.mise.toml` once. Do this right after `git worktree add`, before `just setup`.

**Watch host memory, not just `target/` locks.** Rust builds are bursty — cargo runs one `rustc`
per core (18 here) and heavy crates hold hundreds of MB each at peak, so N concurrent builds can
spike well past free RAM and the kernel SIGKILLs `rustc` (cargo then kills the sibling jobs, giving
a wall of `signal: 9` on unrelated crates — not a real compile error). Worktrees + shared sccache
are the main mitigation (cache hits skip the memory-heavy codegen). If you knowingly run a fleet,
cap per-build parallelism with `CARGO_BUILD_JOBS` rather than letting every build grab all cores,
and quit memory-heavy background apps (e.g. OrbStack's VM is only needed for the Docker-gated
`oracle`/`test-pg`/`e2e` recipes — quit it for plain `just test`, which is SQLite-only/no-Docker).

## Conventions

Code/comment/commit/test conventions live in [`docs/agents/conventions.md`](docs/agents/conventions.md).
Definition of done in [`docs/agents/definition-of-done.md`](docs/agents/definition-of-done.md).
Don't reference issue/bug numbers in code comments.
