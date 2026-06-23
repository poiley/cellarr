# CLAUDE.md — cellarr project rules

Project-level instructions for any agent working in `~/repos/cellarr`. These override defaults and
sit alongside the global `~/.claude/CLAUDE.md` (system tools) and the plan in
[`docs/`](docs/README.md). Read [`docs/agents/AGENTS.md`](docs/agents/AGENTS.md) before working.

## 🚫 Rule #1: this project is PRIVATE — never push to a remote

- **Local commits are permitted and encouraged.** Branch, commit, use worktrees freely.
- **Remote pushing is forbidden.** Do not `git push`, do not add/enable a remote, do not create a
  GitHub repo, do not open PRs, do not publish packages (crates.io / npm), do not deploy. The user
  does **not** want this project published anywhere.
- This is enforced defense-in-depth by a committed pre-push hook (`.githooks/pre-push`) that rejects
  all pushes. `just setup` wires it via `git config core.hooksPath .githooks`. Do not disable it.
- If a task seems to require publishing, it is wrong — stop and ask.

## Non-negotiables (from the plan — memorize)

See [`docs/agents/AGENTS.md`](docs/agents/AGENTS.md) for the full list. In short: one static binary /
zero required services (SQLite default); works offline; never corrupt the user's library
(stage→verify→commit→log); keep `/api/v3` compatible; UI is **exclusively** Sacred/SRCL; learn
upstream behavior clean-room, never transcribe code ([`docs/agents/legal-and-licensing.md`](docs/agents/legal-and-licensing.md)).

## Toolchain

Managed by **mise** (see `.mise.toml`): Rust (stable) and Node. Run `just setup` once to install
toolchains, wire the git hook, and install web deps. Rust/Cargo are **not** globally installed on
this machine — use the project's mise-managed toolchain, not a system one.

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

## Conventions

Code/comment/commit/test conventions live in [`docs/agents/conventions.md`](docs/agents/conventions.md).
Definition of done in [`docs/agents/definition-of-done.md`](docs/agents/definition-of-done.md).
Don't reference issue/bug numbers in code comments. Commit locally; never push.
