# Spec: cellarr-indexers

## Responsibility
Discover candidate releases. Implements the `Indexer` trait for **Torznab/Newznab** natively and a
**Cardigann YAML engine** that interprets community indexer definitions at runtime. Normalizes all
results into `cellarr-core::Release`.

## Allowed dependencies
Internal: `cellarr-core`. External: `reqwest`, `tokio`, an XML parser, an HTML/CSS+XPath selector
lib (for Cardigann), `governor` (rate limiting), `serde`, `thiserror`.

## Public interface
- `Indexer` impls: `TorznabIndexer`, `NewznabIndexer`, `CardigannIndexer` (constructed from a parsed
  definition).
- `caps()` — fetch & cache capabilities; `search(query)` / `tvsearch` / `movie` / etc.
- A Cardigann definition loader/parser (loads from a user-configured source, **not** vendored).
- An optional outbound Torznab/Newznab proxy endpoint (so external apps can use cellarr's indexers).

## Behavior
- **Call `t=caps` first**; read supported modes/params/categories from caps — never hardcode.
- Per-host rate limiting via `governor`, conservative defaults; respect retry/backoff and bans.
- Cardigann definitions are consumed from a source the user points at; **do not commit the
  unlicensed `Prowlarr/Indexers` YAML into this repo** ([agents/legal-and-licensing.md](../agents/legal-and-licensing.md)).
- All adapters normalize to `Release`; downstream stages are indexer-agnostic.

## Test obligations
- **Record/replay**: recorded `caps` + search responses (Torznab, Newznab, and several real-world
  Cardigann sites) → asserted normalization. No live indexers in CI.
- Cardigann engine: selector/field/template extraction tested against recorded HTML/JSON.
- Rate-limiter behavior tested (no bursts beyond configured limits).
- A drift suite (opt-in) checks recorded shapes against live endpoints.

## References
[06-integrations.md](../06-integrations.md), [13-upstream-repos.md](../13-upstream-repos.md),
[11-testing.md](../11-testing.md).
