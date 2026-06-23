# AGENTS.md — how to work on cellarr

This is the operating manual for any agent (or human) building cellarr. Read it fully before
touching anything. It is short on purpose. The rules here override convenience.

## The mental model

cellarr is being built by **swarms of agents working in parallel against specs and tests**. You
are one of them. You will rarely have the whole system in your head. That is fine — the system
is designed so you don't need it:

- Every crate has a **spec** in [`docs/specs/`](../specs/) that defines its job, its public
  interface, and its **test obligations**. You implement against the spec.
- Crates talk to each other through **traits and typed messages**, not shared mutable state.
  You can build and test your crate against trait mocks while another agent builds the crate
  behind that trait.
- **Tests are the contract.** A feature is not done because the code looks right. It is done
  when the tests that encode its required behavior pass — including the corpus tests and, where
  applicable, the differential oracle. See [`../11-testing.md`](../11-testing.md).

## The non-negotiables (memorize these)

If a task would make you violate one of these, stop and flag it — the task is wrong, not the rule.

1. **One static binary, one container, zero required external services.** Default to SQLite and
   embedded everything. Never add a hard dependency on Redis, Postgres, Elasticsearch, a message
   broker, or a cloud service.
2. **Works fully offline** except for the network calls inherent to the job. No feature may
   *require* a cloud LLM or paid SaaS. Optional enhancements (e.g. a hosted LLM parser fallback)
   must degrade to a local/offline path.
3. **Never corrupt the user's library.** Any code that moves, renames, overwrites, or deletes a
   file must follow the stage→verify→commit→log discipline in [`../03-pipeline.md`](../03-pipeline.md).
   Destructive operations driven by inference require a confidence gate; a hallucinated parse must
   never overwrite the wrong file.
4. **Ecosystem compatibility is a feature.** Do not break the `/api/v3` compatibility shim.
5. **UI is composed exclusively from Sacred/SRCL components.** No bespoke UI primitives, no other
   component libraries, no hand-rolled CSS design system. See [`../10-ui.md`](../10-ui.md).
6. **Clean-room the upstream knowledge.** Learn behavior from the *arr source; do **not**
   transcribe its code. Reuse test *vectors* (facts) per [`legal-and-licensing.md`](legal-and-licensing.md).

## The working loop

For any task:

1. **Read** the relevant spec in `docs/specs/` and the docs it links. Read the upstream notes in
   [`../13-upstream-repos.md`](../13-upstream-repos.md) if your crate touches upstream behavior.
2. **Find the tests first.** If the corpus/fixtures for your behavior exist, run them (they will
   fail — that's the target). If they don't exist yet, *write them first* from the spec and from
   the upstream fixtures, and get them reviewed before implementing.
3. **Implement** the smallest slice that turns a red test green. Prefer many small typed
   functions over large ones. Match the conventions in [`conventions.md`](conventions.md).
4. **Verify** by running the crate's tests *and* the gates listed in
   [`definition-of-done.md`](definition-of-done.md). For anything observable, verify behavior, not
   just compilation.
5. **Record** anything non-obvious you learned (a parser edge case, an indexer quirk) in the
   relevant doc or as a corpus comment — the next agent should not have to rediscover it.

## What you may and may not decide

- **You may** choose internal implementation details, helper functions, local data structures,
  and additional tests.
- **You must not** unilaterally change a public trait/interface defined in a spec, the data model,
  the API contract, the database engine policy, or the UI component policy. If a spec is wrong or
  insufficient, propose the change in the spec doc and get it agreed before diverging — other
  agents are building against it right now.

## Reference material

- `reference/` holds cloned upstream repos for **study only**. It is git-ignored and never
  shipped. Read it; do not copy from it (see the clean-room rules).
- The Sacred UI library is at `reference/www-sacred/`. Its own `components/AGENTS.md` is the
  canonical component catalog. Its sources are also fetchable at
  `https://sacred.computer/llm/components/<Name>.tsx.txt`.

## When you are stuck or find a contradiction

Do not guess on anything load-bearing (data model, file operations, money/metering, licensing).
Flag the contradiction in the relevant doc and stop, rather than shipping a guess that another
agent will build on. Guessing is fine for reversible internal details; it is not fine for the
non-negotiables.
