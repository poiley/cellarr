# Spec: cellarr-plugins

## Responsibility
Host third-party extensions as sandboxed **WASM components** (`wasmtime`, WIT-defined interfaces):
custom indexers, download clients, notifiers, and metadata sources we don't ship. **Post-v1** — but
the host interfaces are designed now so core traits stay plugin-friendly.

## Allowed dependencies
Internal: `cellarr-core`. External: `wasmtime` (v46+, component-model), `wasmtime-wasi`, `serde`,
`thiserror`. Behind a cargo feature; not in the default minimal build path unless enabled.

## Public interface
- WIT worlds for each extension kind, mirroring the core traits (`Indexer`, `DownloadClient`,
  `MetadataSource`, notifier).
- A host that loads a component, grants only explicit capabilities, and adapts it to the matching
  core trait so the rest of cellarr treats a plugin like any built-in.
- A capability layer (e.g. a narrow host-provided HTTP-fetch) — **no ambient authority**.

## Behavior
- **Sandboxing:** no default capabilities; the host grants specific ones (prefer a narrow HTTP
  capability over `wasi-sockets`). CPU bounded by epoch interruption + fuel; memory by `StoreLimits`;
  fresh instance per invocation for untrusted code.
- Default to **WASIp2 sync** for stability; adopt WASIp3 async selectively (Rust guests).
- A misbehaving/slow/oversized plugin is killed without affecting the daemon.

## Test obligations
- Sample WASM guests (one per extension kind) load and satisfy the trait via the host.
- Sandbox tests: a guest cannot exceed granted capabilities; fuel/epoch limits terminate runaways;
  `StoreLimits` caps memory.
- Failure isolation: a panicking/looping plugin doesn't crash or hang the host.
- Feature-disabled build still compiles and passes all non-plugin tests.

## References
[06-integrations.md](../06-integrations.md), [13-upstream-repos.md](../13-upstream-repos.md).
