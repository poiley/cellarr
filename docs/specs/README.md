# Specs — the crate contracts

Each crate has a spec here. A spec is the **contract** an agent implements against — not the
implementation. Agents build against the spec and its tests, not against each other's code.

## Spec template

Every spec follows this shape:

1. **Responsibility** — one paragraph: what this crate is for, and explicitly what it is *not*.
2. **Allowed dependencies** — which internal crates and major external crates it may use. Anything
   not listed needs agreement (keeps the dependency graph clean and acyclic).
3. **Public interface** — the traits/types it exposes (described in prose + minimal signatures).
   This is the seam other crates build against; changing it needs agreement.
4. **Behavior** — the rules it must obey, including the relevant non-negotiables.
5. **Test obligations** — exactly what must be tested for the crate to be "done"
   ([../agents/definition-of-done.md](../agents/definition-of-done.md)).
6. **References** — the docs that govern it.

## Index

| Crate | Spec | Role |
|-------|------|------|
| `cellarr-core` | [cellarr-core.md](cellarr-core.md) | shared types, traits, pipeline state machine |
| `cellarr-db` | [cellarr-db.md](cellarr-db.md) | persistence, writer-actor, migrations |
| `cellarr-parse` | [cellarr-parse.md](cellarr-parse.md) | release-name parser |
| `cellarr-decide` | [cellarr-decide.md](cellarr-decide.md) | profiles + custom formats + decision fn |
| `cellarr-media` | [cellarr-media.md](cellarr-media.md) | per-media-type modules |
| `cellarr-indexers` | [cellarr-indexers.md](cellarr-indexers.md) | Torznab/Newznab + Cardigann engine |
| `cellarr-download` | [cellarr-download.md](cellarr-download.md) | download client adapters |
| `cellarr-fs` | [cellarr-fs.md](cellarr-fs.md) | library file operations (the dangerous crate) |
| `cellarr-jobs` | [cellarr-jobs.md](cellarr-jobs.md) | scheduler, retries, rate limits |
| `cellarr-meta` | [cellarr-meta.md](cellarr-meta.md) | metadata service (Skyhook rebuild) |
| `cellarr-llm` | [cellarr-llm.md](cellarr-llm.md) | inference fallback (local-first) |
| `cellarr-plugins` | [cellarr-plugins.md](cellarr-plugins.md) | WASM plugin host |
| `cellarr-api` | [cellarr-api.md](cellarr-api.md) | REST/WS + /api/v3 shim |
| `cellarr-migrate` | [cellarr-migrate.md](cellarr-migrate.md) | import from existing *arr DBs |
| `cellarr-cli` | [cellarr-cli.md](cellarr-cli.md) | the daemon binary; wiring |
| `web/` | [web-ui.md](web-ui.md) | the SRCL-only frontend |

> Code signatures in specs are **illustrative**, to pin intent and vocabulary. The authoritative
> form is the actual trait in `cellarr-core` plus the tests. Keep signatures minimal — this is a
> plan, not an implementation.
