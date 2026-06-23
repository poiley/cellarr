# cellarr — build status

A snapshot of what is actually implemented, tested, and deferred. The design lives in
[`docs/`](docs/README.md); this file is the honest "where the code is" companion.

## Summary

**v1 build complete and green.** ~29k lines of Rust across 15 crates + a Next.js/SRCL web app.
The single daemon boots with zero config, runs the full acquisition pipeline, serves the native
and `/api/v3`-compatible APIs over REST + WebSocket, and serves the theme-aware SRCL UI — all from
one binary. Verified in a real browser (light/dark via system default, live API data).

Gate: `just ci` passes — **cargo 303 tests + web 92 tests**, `clippy -D warnings` clean, `fmt`
clean, SRCL-only UI lint clean.

## Implemented & tested

| Area | Crate / dir | Notes |
|------|-------------|-------|
| Shared types, traits, pipeline state machine | `cellarr-core` | media-type-agnostic `ContentRef`/`Coordinates`; seam traits |
| Release-name parser | `cellarr-parse` | extractor pipeline; ~120 corpus vectors; proptest no-panic |
| Decision engine | `cellarr-decide` | quality profiles + custom formats + precedence; TRaSH import |
| Persistence | `cellarr-db` | SQLite (WAL) + writer-actor + migrations + repos |
| Library file ops | `cellarr-fs` | stage→verify→commit→log import with crash-safety tests |
| Metadata service | `cellarr-meta` | TMDb/TheTVDB sources, moka cache, scene-mapping; record/replay |
| Media modules | `cellarr-media` | movie + TV; anime absolute→episode remap |
| Indexers | `cellarr-indexers` | Torznab/Newznab (caps-first) + Cardigann engine; record/replay |
| Download clients | `cellarr-download` | qBittorrent/SABnzbd/NZBGet; version-divergence fixtures |
| Pipeline executor + scheduler | `cellarr-jobs` | real end-to-end discover→import test (movie + TV) |
| Migration | `cellarr-migrate` | Radarr + Sonarr SQLite import; recognize-in-place |
| API | `cellarr-api` | native `/api/v1` REST + WS/SSE + `/api/v3` shim + OpenAPI + embedded UI |
| Daemon | `cellarr-cli`, `meta-service` | zero-config boot, graceful shutdown, subcommands |
| Web UI | `web/` | Next.js + vendored SRCL; all v1 screens; light/dark/system theming |
| Inference fallback | `cellarr-llm` | feature-gated, local-first, default off |
| Plugin host | `cellarr-plugins` | feature-gated wasmtime Component-Model host |

## Verified end-to-end (real, not just unit tests)

- Daemon boots zero-config, serves `/api/v1/system/status` and `/api/v3/system/status`, SIGTERM → clean shutdown.
- `cellarr migrate` imports the Radarr+Sonarr fixtures into one unified library set (recognize-in-place, 0 file ops).
- Browser: UI renders in light **and** dark via OS preference (no flash), per-route navigation, live API data (imported Movies + TV libraries).
- `cellarr-jobs` end-to-end test drives a movie and a TV episode discover→imported (files land on disk at renamed paths; grab→Imported; decision_log + history written); a junk release is rejected with a logged reason and no file moved.

## Deferred (documented, not silently missing)

- **Postgres backend** — opt-in/post-v1 (SQLite is the v1 non-negotiable default). The `postgres`
  cargo feature wires the sqlx driver; the repository layer is SQLite-only for now. `just test-pg`
  reports the deferral; the harness is preserved behind `CELLARR_ENABLE_PG_TESTS=1`. See
  [`docs/08-database.md`](docs/08-database.md).
- **Music & books media types** — designed for in the model; post-v1 per [`docs/00-vision.md`](docs/00-vision.md).
- **Differential-oracle parity runs** — the harness/plan exists ([`docs/11-testing.md`](docs/11-testing.md));
  running real pinned Sonarr/Radarr in Docker to measure parity % is a quality activity not yet executed.
- **Live-service smoke suites** — integrations are tested via record/replay fixtures (offline);
  opt-in live drift suites against real indexers/clients/metadata are not wired.
- **LLM & WASM plugins** — implemented but feature-gated off by default (kept out of the lean single binary).
- Some integration coverage (Cardigann breadth, more download clients, richer `/api/v3` surface) is
  first-cut and intended to grow against the test corpora.

## Build & run

```sh
just setup     # mise toolchains + git hook + web deps
just ci        # full gate (cargo + web)
just dev       # run the daemon + UI for this worktree (see docs/16)
# or directly:
mise exec -- cargo run -p cellarr-cli      # daemon (zero-config), serves UI + API
mise exec -- cargo run -p cellarr-cli -- migrate <radarr.sqlite> <sonarr.sqlite>
```

Private project: local commits only, **never pushed** (enforced by `.githooks/pre-push`).
