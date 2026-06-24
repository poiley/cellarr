# cellarr — build status

A snapshot of what is actually implemented, tested, and deferred. The design lives in
[`docs/`](docs/README.md); this file is the honest "where the code is" companion.

## Summary

**v1 build complete and green.** ~29k lines of Rust across 15 crates + a Next.js/SRCL web app.
The single daemon boots with zero config, runs the full acquisition pipeline, serves the native
and `/api/v3`-compatible APIs over REST + WebSocket, and serves the theme-aware SRCL UI — all from
one binary. Verified in a real browser (light/dark via system default, live API data).

Gate: `just ci` passes — **cargo 529 tests + web 92 tests**, `clippy -D warnings` clean, `fmt`
clean, SRCL-only UI lint clean.

**Test-hardening is complete and verified bulletproof** — curated corpus 100% must-pass, ratcheted
upstream self-parity, at-scale differential + TRaSH-CF oracles, **100% mutation score on
cellarr-fs and cellarr-decide** (79.6% on cellarr-parse), **95.4% region / 96.2% line coverage** on
the three critical crates, plus proptest invariants and a libFuzzer no-panic target. The full
strategy and every hard number live in **[`docs/parity/TESTING.md`](docs/parity/TESTING.md)**.

## Implemented & tested

| Area | Crate / dir | Notes |
|------|-------------|-------|
| Shared types, traits, pipeline state machine | `cellarr-core` | media-type-agnostic `ContentRef`/`Coordinates`; seam traits |
| Release-name parser | `cellarr-parse` | extractor pipeline; 131 curated + 1,555 upstream corpus vectors; proptest + libFuzzer no-panic |
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
- **Live stack run (2026-06-23, against the user's real Prowlarr + Transmission + NAS).** The
  prebuilt daemon was driven end to end: configured a Transmission download client, a Torznab indexer
  pointed at the user's real Prowlarr (TPB feed), and a remote-path mapping, then added *Big Buck
  Bunny* (TMDb 10378) and triggered a search. **OBSERVED live:** cellarr really queried Prowlarr (22
  TPB releases returned), parsed/identified the movie, reached a **Grab** verdict, persisted a grab
  row, and called the **real Transmission `torrent-add` RPC**. Furthest stage reached = **grabbed**.
  The grab then **failed** at the magnet-redirect boundary: Prowlarr's `/download` 301-redirects into
  a `magnet:` URI and cellarr's Torznab adapter passes only the `<enclosure>` HTTP URL to the client
  (it ignores the magnet sitting in `<guid>`; see `crates/cellarr-indexers/src/feed.rs` line ~53,
  `download_url = self.enclosure_url.or(self.link)`), so `torrent-add` got `Moved Permanently (301)`
  and could not fetch the torrent. The daemon's own DB recorded **2 grab rows status=failed**, **2
  `download_failed` history events**, **0 `media_file` (no import)**; nothing was written to the NAS
  and no torrent was left on the user's Transmission. So **search→decide→grab→Transmission-RPC is
  proven live, but the real byte-transfer + import did not complete** — a genuine integration gap
  (magnet-redirect indexers like TPB), not a config error. The hermetic e2e (below / `pipeline_e2e.rs`)
  still covers download→import with a direct-`.torrent` / pre-staged completed dir.

## Deferred (documented, not silently missing)

- **Postgres backend** — opt-in/post-v1 (SQLite is the v1 non-negotiable default). The `postgres`
  cargo feature wires the sqlx driver; the repository layer is SQLite-only for now. `just test-pg`
  reports the deferral; the harness is preserved behind `CELLARR_ENABLE_PG_TESTS=1`. See
  [`docs/08-database.md`](docs/08-database.md).
- **Music & books media types** — designed for in the model; post-v1 per [`docs/00-vision.md`](docs/00-vision.md).
- **Differential-oracle parity** — DONE for the parser: pinned Sonarr 4.0.17 + Radarr 6.2.1 diffed
  against cellarr over the corpus → **90% exact** (up from 76.7%), mechanical gaps fixed, all
  remaining gaps catalogued in [`docs/parity/`](docs/parity/README.md). Reproduce with `just oracle`.
  Still open: the **CF-score / decision oracle** (import a TRaSH set both sides, diff scores) and the
  **identify/matching oracle** (needs populated libraries) — see [`docs/parity/decision-gaps.md`](docs/parity/decision-gaps.md).
- **Live-service smoke suites** — integrations are tested via record/replay fixtures (offline);
  opt-in live drift suites against real indexers/clients/metadata are not wired.
- **Magnet-redirect indexers (e.g. The Pirate Bay via Prowlarr)** — the Torznab adapter
  (`cellarr-indexers/src/feed.rs`) uses only the `<enclosure>`/`<link>` download URL and does not
  fall back to the `magnet:` URI in `<guid>` or follow a `/download` → `magnet:` 301 redirect. For
  trackers that only offer magnets behind a redirect, the live grab reaches the download client but
  `torrent-add` cannot fetch the URL (observed `Moved Permanently (301)` in the 2026-06-23 live run).
  Direct-`.torrent` indexers are unaffected. Fix = use the `<guid>`/`infohash` magnet or resolve the
  redirect (deferred, not a research problem).
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
