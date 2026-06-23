# Conventions

Conventions that keep a swarm's output coherent. Follow them; they are checked in CI where possible.

## Rust

- **Edition & toolchain:** latest stable Rust, managed by **mise** via `.mise.toml` (`rust =
  "stable"`) — *not* a `rust-toolchain.toml`. Cargo/rustc are not globally installed on dev machines;
  invoke them through the mise-managed toolchain (e.g. `mise exec -- cargo …`). No nightly features in
  shipping crates.
- **Formatting:** `cargo fmt` (default rustfmt). CI runs `cargo fmt --check`.
- **Lints:** `cargo clippy --all-targets -- -D warnings`. Warnings are errors in CI.
- **Errors:** libraries use `thiserror` for typed errors; the binary uses `anyhow` at the top level.
  Never `unwrap()`/`expect()` on fallible runtime paths (tests may). No `panic!` on user-reachable
  input — the parser must never panic (property-tested).
- **Async:** `tokio`. Never block the reactor — CPU work goes to `rayon`/`spawn_blocking`, file IO
  off the async threads.
- **Public surface:** keep crate public APIs minimal and trait-centric (the seams). Internal types
  stay `pub(crate)`. Document every public item with a doc comment.
- **No global mutable state.** Dependencies are passed in (constructor injection), which is also what
  makes mocking and parallel agent work possible.
- **Feature flags:** keep optional heavy deps (Postgres, wasmtime, llm providers) behind cargo
  features so the default build stays lean. The default build must satisfy the single-binary
  non-negotiable.

## Comments

- Comments explain **why**, not **what**. If the code reads clearly, write no comment, and delete
  self-documenting comments on sight.
- **Never reference issue/bug/ticket numbers in code comments** (no `// fix for #123`,
  `// JIRA-...`). They're meaningless to future readers; let git history carry traceability. Write
  the *reason*, not the ticket.
- A regex or heuristic that encodes a non-obvious release-naming fact **should** have a short
  why-comment and a corresponding corpus vector.

## Tests

- Co-locate unit tests with code; integration tests in `tests/`. Table-driven tests use **rstest**
  `#[case(...)]`, which maps directly onto corpus vectors.
- Corpus-backed components load vectors from `/corpus`; adding behavior means adding vectors.
- Integrations use **record/replay** fixtures; no live services in CI.
- Property tests (**proptest**) for the parser and scoring invariants.
- A test's name states the behavior it pins. A failing test should read like a spec violation.

## TypeScript / web

- The `web/` app follows SRCL's own conventions (see `reference/www-sacred/AGENTS.md`): TS strict,
  `tsc --noEmit` clean, vitest for component tests.
- **UI components come only from SRCL** — enforced by the SRCL-only lint ([../10-ui.md](../10-ui.md)).
- Match SRCL's comment style in vendored/adjacent code if vendoring.

## Git & commits

- **Branch per task**; never commit directly to the default branch.
- Small, focused commits; the diff should match the task. Don't bundle unrelated changes.
- Conventional-style messages (`feat:`, `fix:`, `test:`, `docs:`, `refactor:`) with a crate scope,
  e.g. `feat(parse): extract HDR flags`.
- Commit the offline `.sqlx` query metadata when you change SQL.
- **Commit or push only when asked / per the swarm's orchestration.** Don't open PRs unprompted.
- Co-authorship trailer per the environment's policy when committing on behalf of an agent.

## Naming

- Crates: `cellarr-<area>`. Binaries: `cellarr` (daemon), `cellarr-meta` (standalone metadata).
- Types use the domain vocabulary in [../15-glossary.md](../15-glossary.md). Don't invent synonyms
  for `ContentRef`, `Coordinates`, `MediaModule`, etc. — consistency across crates matters more than
  local preference.

## Documentation

- If you change behavior described in a `docs/` file, update that doc in the same change. The docs
  are the plan of record; drift between docs and code is a defect.
