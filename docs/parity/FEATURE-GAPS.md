# Feature gaps → Sonarr/Radarr drop-in parity (Tier A + B program)

Tracks the remaining feature work to be a no-regression Sonarr+Radarr replacement. The core
acquisition loop, decision engine (TRaSH CFs), self-heal, and the UI are done (see
[REPLACEMENT-ROADMAP.md](REPLACEMENT-ROADMAP.md), [TESTING.md](TESTING.md)). These are the
management/integration surfaces that make it a daily driver.

Executed as a **sequence of feature packs**, each on a clean committed base (backend → frontend →
verify, with tests), to avoid cross-feature conflicts on shared files (the `/api/v3` router, the
migration sequence, the settings UI, the API client). SRCL-only for all UI; clean-room; design
patterns + DoD maintained.

Music/books (Lidarr/Readarr) are a separate media-type axis and are **out of scope** for this program.

| Pack | Scope | Status |
|------|-------|--------|
| **1 — Metadata & artwork & calendar** | Persist year/overview/runtime + episode air dates on identify (closes the content-detail blanks); download/cache/serve artwork (MediaCover); `.nfo` export on import (Kodi/Jellyfin); real Calendar/Upcoming view | ✅ done |
| **2 — Notifications & media-server** | Provider set (Discord/Telegram/email-SMTP/custom-script/generic-webhook; Pushover/Slack/etc. deferred) + Plex/Jellyfin/Emby rescan-on-import; fire on grab/import/upgrade/health; `/api/v3/notification` CRUD+test+schema; Settings>Notifications UI. Also fixed a systemic v3-id JS-precision bug (53-bit mask). | ✅ done |
| **3 — Library management** | Delete movie/series (+files) + recycle bin; manual import (scan→match→import loose files); per-season/episode monitoring options; wire bulk delete | ✅ done (3a delete/recycle/bulk-delete; 3b manual-import + per-episode monitoring + movie-naming year/graceful-token + episode monitor tree) |
| **4 — Decision & quality depth** | Custom-format editor (author/edit specs); naming-config UI + token coverage + permissions + extra-file import; delay profiles | ✅ done (4a CF editor + delay profiles; 4b naming-config UI + chmod/chown permissions + extra-file/subs import) |
| **5 — Download clients & indexers** | Deluge + rTorrent adapters; per-indexer priority / seed criteria / freeleech flags; Cardigann depth | ✅ done (Deluge JSON-RPC + rTorrent XML-RPC adapters [record/replay; live-validation deferred — no server]; indexer priority tie-break + min-seeders/seed-criteria + freeleech gating. Cardigann definition breadth = ongoing long-tail. Follow-up: download-client list-row display label shows qBittorrent for non-qBit clients [cosmetic; data correct].) |
| **6 — Lists & queue & ops & auth** | Import lists fully wired (Trakt/TMDb/Plex/IMDb) + collections; queue management (remove / manual-import / category); backup/restore + log viewer + health breadth; authentication / user accounts | ✅ done (6a import lists [TMDb/Trakt/Plex/IMDb + TMDb collections, sync safeguard, idempotent dedup] + queue mgmt; 6b-ops backup/restore + log viewer + health breadth; 6b-auth configurable single-user None/Forms/Basic, /api/v3 stays apikey) |

**✅ Tier A + B feature-completeness program: COMPLETE.** All 6 packs shipped, tested, and committed locally (gated by `just ci`). Each pack was independently verified end-to-end (live daemon + browser) with an adversarial fake-green hunt; real bugs found by that process were fixed with regression tests (systemic v3-id JS-precision, movie-naming year, import-list re-sync duplicates, the Forms `/login` 405, a test-isolation flake).

**Deferred long-tail (intentional, documented — not blockers):**
- **Cardigann definition breadth** — the engine works; broad per-tracker definition coverage is ongoing.
- **Postgres backup/restore** — SQLite (the default) is fully supported; Postgres path marked `// TODO`.
- **Live download-client/list validation** — Deluge/rTorrent and Trakt/Plex/IMDb are record/replay- and contract-tested but not validated against real live servers/credentials (none available); honestly deferred.
- **Cosmetic:** download-client list-row shows "qBittorrent" label for a non-qBit client (persisted data + edit form correct); a valid-magic-but-corrupt-DB restore upload returns 500 vs 400 (live DB stays safe).
- **Pushover/Slack notification providers** — long-tail beyond the shipped Discord/Telegram/email/script/webhook set.

"In full" = all the features with the common/important variants + tests; genuinely long-tail
breadth (every Cardigann definition, every niche download client/notification provider) is filled
incrementally and noted where partial. Anything that can't be completed is deferred **with a reason +
TODO**, never faked.
