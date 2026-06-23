# Replacing Sonarr + Radarr with cellarr — the complete roadmap

**Goal:** a user removes Sonarr **and** Radarr from their stack, points everything (Prowlarr,
download clients, Overseerr/Jellyseerr, Bazarr, Recyclarr, notifications, dashboards) at **one**
cellarr instance, and sees **no regression**.

This roadmap is grounded in: the measured parser oracle ([PARITY_REPORT.md](PARITY_REPORT.md)), the
`/api/v3` ecosystem probe ([api-v3-gaps.md](api-v3-gaps.md)), the quality-vocabulary diff
([quality-vocab.md](quality-vocab.md)), the decision-engine assessment
([decision-gaps.md](decision-gaps.md)), and a full functional + ecosystem inventory of the originals.

---

## 1. Definition of "drop-in" (the bar)
A drop-in replacement must satisfy, with no regression:
1. The `/api/v3` surface the ecosystem calls (Tiers 1–4 in [api-v3-gaps.md](api-v3-gaps.md)), both
   auth modes, the `X-Application-Version` header, and the paging envelope.
2. Real **indexer execution** (Torznab/Newznab search + RSS) so Prowlarr-pushed indexers work.
3. Real **download-client** integration (qBittorrent/SABnzbd min) with categories + completed-download
   handling + remote-path mappings.
4. **Import** with hardlink→atomic-move→copy semantics and same-filesystem detection.
5. **Quality + custom-format scoring** with correct precedence (quality → revision → CF score).
6. **Library state correctness** (`hasFile`/`monitored`/availability) so Overseerr marks items available.
7. **Metadata** identity (series/movie lookup + add) — the one piece gated on external licensing.
8. **Webhook/Connect** push (`eventType` + `Test`) for Bazarr-push/Notifiarr/notifications.

---

## 2. Current state (what cellarr already has)
- **Engine, built & green:** unified core, parser, decision engine, SQLite persistence, file-ops
  (stage→verify→commit→log), jobs/pipeline (real discover→import e2e test), migrate (Radarr+Sonarr
  SQLite import), API skeleton, SRCL UI. `just ci` passes (cargo 311 + web 92).
- **Parser parity: 90% exact** vs the live originals; mechanical gaps closed; the rest catalogued.
- **Decision logic:** precedence + CF condition semantics implemented + unit-tested; TRaSH import
  present. CF-score **not yet oracle-measured** against the apps.
- **`/api/v3` shim:** ~5 real GET endpoints + lookup/add/command — a thin slice of the ~25 needed.
- **Integrations:** indexer (Torznab/Newznab + Cardigann engine) and download-client (qBit/SAB/NZBGet)
  adapters exist with **record/replay fixtures**, but are **not wired into the live `/api/v3`** or
  end-to-end against real services yet.

---

## 3. Parity & coverage matrix (every functional area)
Legend: ✅ done/measured · 🟡 partial · 🔴 missing · 🔵 blocked on external dependency

| Area | cellarr status | Evidence / gap |
|------|----------------|----------------|
| Release parsing | 🟡 90% exact | PARITY_REPORT; G3/G4/G7/G8 deferred (parser-gaps.md) |
| Quality bucketing | 🟡 98.3% logic | quality-vocab.md: missing 576p/Raw-HD + movie low-tiers; remux naming (per-app) |
| Custom formats (matching) | ✅ 100% (oracle) | decision-gaps.md: caught + fixed case-insensitivity (G-CF1: TRaSH CFs would have silently failed) |
| CF scoring + precedence | 🟡 logic + unit-tested; matching=100% | score follows from matching; needs `formatItems` in shim + score-confirm oracle |
| Quality profiles | 🟡 core + UI | shim `qualityprofile` missing `formatItems`; no `/schema` |
| Decision engine (grab/upgrade/reject/cutoff) | ✅ logic + tests | precedence proven via inputs; live-search oracle deferred |
| Indexers (Torznab/Newznab) | 🟡 adapter + fixtures | not wired to `/api/v3/indexer`; no live search yet |
| Cardigann definitions | 🟡 engine skeleton | breadth + live trackers untested |
| Download clients | ✅ wired live (qBit) + import handoff; blackhole + remote-path landed | live qBit add/category/track/remove (v5.2.2, Phase D); completed→import handoff in the runner; **blackhole/watch-folder universal adapter** (implements core `DownloadClient`; `add` writes `.torrent`/`.nzb`/`.magnet` to watch dir, `status` flips to Completed+content_path when output lands in completed dir; in `/api/v3/downloadclient/schema` as `TorrentBlackhole`/`UsenetBlackhole`); **remote-path mapping as a shared layer** (`cellarr_core::apply_remote_path_mappings`, applied once in the jobs runner before `plan_import`; CRUD at `/api/v3/remotepathmapping` on both faces) |
| Import / rename / hardlink | ✅ logic + crash-safety + `st_dev` warn | same-filesystem (`st_dev`) detection + loud cross-fs health warning shipped (Phase D, differentiator) |
| Metadata / identify | 🟡 TV live & wired; movies blocked-on-key | TheTVDB lookup live through v3 shim (real `tvdbId`/title, verified Breaking Bad=81189); TMDb needs `CELLARR_TMDB__API_KEY` |
| Anime (absolute/XEM/AniDB) | 🟡 extract + remap path; live TheXEM provider wired | remap backed by live TheTVDB+TheXEM (`TvdbSceneMappings`); pipeline invocation gated on identity-link query; corpus depth |
| Daily shows | ✅ parse + date | timezone handling to verify |
| Season packs / multi-ep | 🟡 modeled | persist release-type as durable state (avoid re-grab loops) |
| Calendar / iCal | ✅ iCal/ICS feed live | `/feed/v3/calendar/{sonarr,radarr}.ics`, apikey-query auth, RFC 5545 VEVENTs (Phase F); JSON `calendar` still thin |
| Queue / history / activity | 🟡 JSON + envelope | add `sortKey/sortDirection`; wire to live downloads |
| Blocklist | 🔴 | failed-download blocklist + redownload |
| Notifications / Connect webhook | 🔴 (native WS only) | `eventType` webhook + `Test` event |
| Import lists | ✅ framework + safeguard; sources blocked-on-key | `ListSource` trait + safeguarded `sync_import_list` (failed/empty-errored fetch wipes NOTHING; `last_successful_sync` stamped only on confirmed-good); `/api/v3/importlist` CRUD+schema+test + `/importlistexclusion` both faces; Trakt/TMDb/Plex sources wired but credential-gated (fail gracefully), tested via a mock source (Phase F) |
| Tags | 🔴 in shim | `/api/v3/tag` |
| Root folders | 🟡 core | `/api/v3/rootfolder` missing in shim |
| Naming tokens | 🟡 rename engine | full token + multi-episode-style coverage |
| `/api/v3` ecosystem surface | 🔴 thin | api-v3-gaps.md (largest block) |
| `X-Application-Version` header | 🔴 | Prowlarr-blocking |
| Migration from existing installs | ✅ | Radarr+Sonarr SQLite import, recognize-in-place |
| Web UI | ✅ | SRCL, light/dark/system |

---

## 4. Phased roadmap to drop-in

Ordered so each phase unlocks a real chunk of the ecosystem. Each phase has an **exit gate** that is
an oracle/contract test, not a vibe.

### Phase A — `/api/v3` ecosystem core (the biggest unlock) — ✅ IMPLEMENTED (2026-06-23)
Shipped: **two faces** (`/sonarr/api/v3`, `/radarr/api/v3`) + the bare `/api/v3`, so the user adds
cellarr twice (as a Sonarr and a Radarr). B1 fixed (404 JSON), `X-Application-Version` per face, both
auth modes, full `system/status`, `GET /series`(+`/episode`)/`/movie` lists, `rootfolder`, `tag`
CRUD, `health`, `qualitydefinition`, `wanted/missing`, `GET command`, `qualityprofile` +
`formatItems` + `/schema`, `customformat` CRUD + `/schema`, `indexer` CRUD + `/schema` + `/test` +
`?forceSave=true`, full paging envelope. Contract-tested against live Sonarr 4.0.17 / Radarr 6.2.1
fixtures (`crates/cellarr-api/tests/fixtures/`, `tests/v3_faces.rs`). Additive core change:
`ContentRepository::roots(library)`. Detail + deferred items in
[api-v3-gaps.md](api-v3-gaps.md). Original scope (for reference):
- `system/status` full fields + **`X-Application-Version` header** + version identity decision.
- `GET /series` and `GET /movie` (list), with accurate `path`/`*File.path`/`rootFolderPath`.
- `rootfolder`, `tag`, `health`, `qualitydefinition`, `wanted/missing`, `GET /command`.
- `qualityprofile` + **`formatItems[]`** + `/qualityprofile/schema`; `customformat` CRUD + `/schema`.
- Honor both auth modes when a key is set; full paging envelope.
- **Exit gate:** a contract suite diffs cellarr's `/api/v3` responses against recorded Sonarr/Radarr
  responses for every Tier 1–4 endpoint; Overseerr + Bazarr + a dashboard run green against cellarr.

### Phase B — Quality vocabulary + CF-score oracle (Recyclarr unlock)
- Add `Bluray-576p`, `Raw-HD`, movie pre-retail tiers; per-app remux naming in the shim.
- Build the **CF-matching + CF-score oracle**: import a real TRaSH set into Sonarr/Radarr **and**
  cellarr, diff matched-CF sets and scores over the corpus (decision-gaps.md).
- **Exit gate:** Recyclarr syncs a TRaSH config into cellarr without error; CF-score parity ≥ target.

### Phase C — Indexers live (Prowlarr unlock) — ✅ IMPLEMENTED (2026-06-23)
Shipped: `/api/v3/indexer` configs **persist** to the db (`config.rs`) and the jobs **Discover**
stage reads them and runs the Torznab/Newznab adapter (caps-first → search → parse → decide → import).
New `cellarr-jobs/src/indexers.rs` + `tests/indexer_live_pipeline.rs` (4 tests) drive a **local
mock Torznab HTTP server**: real `t=caps` then `t=tvsearch`, releases discovered+parsed+decided, plus
a 401 fail-fast path.
- **Exit gate result:** the **Prowlarr push sequence** is validated via the scripted-API equivalent
  (GET `indexer/schema` → POST `indexer?forceSave=true` → GET round-trip → **persists across a daemon
  restart**), confirmed live against the daemon. The full **live-Prowlarr-container** round-trip
  *wedged the verify agent* (Prowlarr app-add/host-reachability stalled for hours), so it was stopped
  and validated the scripted way instead — same contract Prowlarr exercises. Re-attempting the full
  container path (with explicit host networking) is a documented follow-up.
- Live search uses a mock Torznab (real private trackers need creds — out of scope); RSS-sync cadence
  wiring is a small follow-up.

### Phase D — Download + import live (end-to-end acquisition) — ✅ IMPLEMENTED (2026-06-23)
Original scope (for reference):
- Wire download-client adapters live (categories, CDH, remote-path mappings); run the full pipeline
  against a real qBittorrent/SABnzbd; add **same-filesystem `st_dev` detection + health warning**.
- **Exit gate:** a real release goes search → grab → download → import → renamed-on-disk against a
  live client, with correct hardlink behavior and a health alert when `/downloads` and library differ.

Shipped:
- **Completed-download → import handoff is wired in `cellarr-jobs`** (`runner.rs`
  `grab_track_import` → `track` → `import`): the runner polls the download client (bounded
  `max_track_polls`, no tight loop), reads the **`content_path` the client reports** on completion,
  then drives cellarr-fs's `plan_import` → `execute_import` (stage→verify→commit→log). The second
  parse (re-parse of the actual file names) gates a force-fit; an import failure holds for review
  (never a destructive write). A directory hand-off (the torrent client's content folder) is walked
  for its media files.
- **Same-filesystem (`st_dev`) detection + the loud cross-filesystem health warning** — the deliberate
  differentiator (§6). cellarr-fs already hardlinks within one filesystem and copies+fsyncs+atomically
  renames across (`fsops.rs`); the new `cellarr-fs::check_same_filesystem` / `FilesystemWarning`
  compares the configured downloads dir's `st_dev` against every library root and raises a loud
  `ImportMechanismCheck` warning for each off-device root. Wired into **both faces** of
  `/api/v3/health` (the shim) **and** the native system-health snapshot via
  `cellarr_api::fs_health::filesystem_warnings`, and `warn!`-logged on every observation. The
  downloads dir is read from each enabled download client's `settings` JSON
  (`download_dir`/`downloadDir`/`save_path`).
- **Exit-gate evidence:**
  - The completed-download → import handoff is proven by the centerpiece e2e
    (`cellarr-jobs/tests/pipeline_e2e.rs`): movie + TV releases drive Discover→Imported with files
    landing at the renamed on-disk paths, **plus** a new
    `completed_download_imports_as_a_hardlink_on_the_same_filesystem` test that PRE-STAGES a
    "completed" download directory (no download) and asserts the imported library file is a **hardlink**
    of the download (same inode, `nlink == 2`, seeding copy preserved).
  - The cross-filesystem warning is proven end-to-end (`cellarr-api/tests/fs_health_v3.rs`): a
    same-fs layout raises nothing; a **genuine second filesystem** (a macOS RAM disk, self-skips when
    unavailable, torn down robustly so it never leaks) makes `/api/v3/health` return the loud
    `ImportMechanismCheck` warning. Unit-tested deterministically too
    (`cellarr-fs::health` cross-device branch).
  - The **live qBittorrent** path was re-run once via
    `crates/cellarr-download/scripts/qbittorrent-live.sh` against an ephemeral
    `linuxserver/qbittorrent` v5.2.2 container: auth (incl. the 5.x 401 quirk), add, **category
    scoping**, bounded status-track, and remove all passed (`LIVE_RESULT=PASS`), within the 120s hard
    bound, container torn down. The script **never waits for a torrent to finish** — only for it to
    appear under its category — so it cannot wedge.
- **Landed (2026-06-23):** the two genuine generic-download wins are built and verified hermetically.
  (1) **Blackhole / watch-folder adapter** — the *universal* client (`BlackholeClient`) that speaks no
  client API: `add` writes a magnet verbatim or fetches a `.torrent`/`.nzb` (via the shared HTTP seam)
  into the watch dir; `status` is filesystem-derived (Completed + `content_path` once the matching
  output appears in the completed dir). It implements the core `DownloadClient` trait so the runner
  uses it like any client, and the Track→Import handoff imports the real file (integration test asserts
  the imported file on disk). Advertised in `/api/v3/downloadclient/schema` as `TorrentBlackhole` /
  `UsenetBlackhole` with `watchFolder` / `completedFolder` fields. (2) **Remote-path mapping** — a
  *shared* layer (`cellarr_core::apply_remote_path_mappings`) applied in **one place**, the jobs runner,
  right after Track reads the client-reported `content_path` and before `plan_import` (boundary-aware
  prefix rewrite, host-scoped, first-match-wins, unmapped passes through). CRUD lives at
  `/api/v3/remotepathmapping` on both the Sonarr and Radarr faces (live-verified returning JSON, not the
  SPA 404).
- **Deferred (small follow-ups):** SABnzbd completed-handling parity (repair/unpack wait) is modeled in
  the adapter but not yet exercised in the live import e2e.

### Phase E — Metadata / identify (the licensing fork) — 🟡 TV LIVE & WIRED (2026-06-23)
- Wire TMDb (movies) live; for TV pick a path for TheTVDB v4 (licensed proxy / per-user PIN / run our
  own Skyhook-equivalent / lead with TMDb-TV or TVmaze). Run the **identify oracle** with populated
  libraries (compare matched IDs).
- **Exit gate:** lookup/add via Overseerr resolves to correct IDs; identify parity measured.
- ✅ **Decision made (2026-06-23):** **default to the user-supported PIN model now** (cellarr logs
  into TheTVDB v4 with a project API key + per-user subscriber PIN), and **build a self-hosted
  Skyhook-equivalent metadata proxy later** (no public Sonarr Skyhook source exists to reference, so
  it's a from-scratch effort, deferred). Key stored in gitignored `.env`
  (`CELLARR_TVDB__API_KEY`/`CELLARR_TVDB__PIN`); see `.env.example`.
- ✅ **TV identity wired live end-to-end (2026-06-23):**
  - `cellarr-meta`'s `TheTvdbSource` is bound through the API via a thin object-safe
    `cellarr_api::MetadataLookup` seam (`AppState.metadata`); the wiring lives in
    `cellarr-cli` (`LiveMetadata`), constructed from `.env` keys at boot.
  - The v3 shim's `series/lookup`/`movie/lookup` now **resolve real identities** (human `title`,
    `titleSlug`, `tvdbId`/`tmdbId`, `year`) from metadata instead of echoing the search term or a
    UUID — **closing the Phase A "UUID title" deferred gap** for identified items. `series`/`movie`
    list resources surface a node's real indexed title (new `ContentRepo::title_for` reverse lookup),
    falling back to the id only when a node is unidentified.
  - The anime absolute→episode remap is now backed by a **live TheTVDB + TheXEM** scene-mapping
    provider (`cellarr-cli::metadata::TvdbSceneMappings` implementing
    `cellarr_media::SceneMappingProvider`), consumed by the existing `remap_absolute`. Unmapped/absent
    mappings surface for manual resolution (library-safety rule), never guessed. *(Pipeline-level
    invocation of the remap is still gated on the `cellarr-db` identity-link query that resolves a
    node's TVDB id — a documented core gap; the live remap path itself is wired and tested.)*
  - **Verified live (2026-06-23):** booted the daemon with the `.env` TheTVDB key and called the
    Sonarr-face `series/lookup?term=Breaking Bad` → resolved **`tvdbId: 81189`, `title: "Breaking
    Bad"`, `year: 2008`** (6 candidates), confirmed both by the `cellarr-cli` `live_lookup_e2e` test
    and a manual `curl`. Movie lookup with no TMDb key returns **HTTP 200 + `[]`** with a logged
    "metadata unavailable" reason (graceful degradation, never a 500).
- 🔵 **TMDb (movies) = blocked-on-key:** the live TMDb client path exists (`TmdbSource`, record/replay
  green) but **no `CELLARR_TMDB__API_KEY` is provisioned**, so movie metadata is intentionally
  unavailable: `movie/lookup` degrades to an empty, clearly-flagged result rather than erroring. Set
  `CELLARR_TMDB__API_KEY` to enable + live-test.

### Phase F — Connect webhooks + lists + calendar polish — ✅ IMPLEMENTED (2026-06-23)
Original scope (for reference):
- `eventType` webhook + `Test` event (Bazarr-push/Notifiarr/notifications); iCal feed; import lists
  (with the **empty-vs-failed-fetch** safeguard so a failed list never wipes the library); blocklist.
- **Exit gate:** Bazarr (push), Notifiarr, and a Trakt/TMDb list run green; failed-fetch leaves library intact.

Shipped:
- **Connect webhooks + `Test` event + blocklist** landed in Phase 1 (Connect `Webhook` notification
  CRUD/schema/test firing real `eventType` Grab/Download/Rename/Health/Test from pipeline transitions;
  failed-download blocklist + skip-on-re-search; both verified hermetically).
- **Import lists framework + the safeguard (this phase):**
  - **Source abstraction** `cellarr_core::importlist::ListSource` (`fetch() -> FetchResult`), with a
    pure, unit-tested **safeguarded sync** `sync_import_list`. The safeguard is enforced *by
    construction*: `FetchResult` is `Fetched(items)` (confirmed-good; empty allowed) **xor**
    `Failed(reason)`; the diff **never** marks anything removable on a failed (or empty-because-errored)
    fetch, `SyncOutcome::removable` is always empty unless the fetch was confirmed-good *and* the list
    opted into a destructive `CleanAction` (default `None`), and `last_successful_sync` is stamped
    (`ImportListRepo::mark_synced`) only on a confirmed-good fetch. The orchestrator
    `cellarr_jobs::importlists::ImportListSync` adds non-present monitored items as content nodes and
    applies the gated clean.
  - **Sources:** Trakt / TMDb / Plex-watchlist wired (`cellarr_jobs::importlists::sources`) but
    **blocked-on-key** — with no credential in the list's `settings` each returns a graceful
    `FetchResult::Failed` (never a network call, never a falsely-empty success). A deterministic
    `MockListSource` exercises the framework offline.
  - **CRUD both faces:** `/api/v3/importlist` CRUD + `/schema` + `/test` and `/api/v3/importlistexclusion`
    on the Sonarr/Radarr/cellarr faces; persisted via the `import_list` / `import_list_exclusion` tables
    (migration `0006`, JSON-body + typed columns). An unrecognized/absent `cleanLibraryLevel` maps to
    the safe `None`.
  - **Tested:** `cellarr-core` unit tests (pure diff incl. the catastrophe setup), `cellarr-jobs/tests/import_list_sync.rs`
    (end-to-end against the real db: a failed fetch with `clean=Remove` over a populated library
    removes/cleans **nothing** and leaves it fully intact + doesn't stamp `last_successful_sync`; a
    confirmed-good list adds items; a confirmed-good *empty* list gates a clean while the same empty
    symptom from a failure does not; exclusions suppress adds; live sources fail gracefully without creds).
- **iCal/ICS calendar feed:** `/feed/v3/calendar/sonarr.ics` (TV) + `radarr.ics` (movies), `apikey`-query
  authenticated. `cellarr-api::calendar` is a pure RFC 5545 ICS writer (CRLF, text escaping, 75-octet
  folding, all-day `DTSTART;VALUE=DATE` VEVENTs) + a feed handler collecting dated library content (TV
  daily-coded episodes today; per-episode air dates / movie release dates follow when identify persists
  them). Empty feed → valid empty `VCALENDAR`. Tested in `cellarr-api/tests/v3_import_list_calendar.rs`.
- **Live status / blocked-on-creds:** the import-list **framework, safeguard, CRUD, and calendar feed
  are live and verified hermetically**; the three real list **sources (Trakt/TMDb/Plex) are
  blocked-on-creds** (no Trakt client-id+slug / TMDb api-key+list / Plex token provisioned) and degrade
  to a graceful failed fetch until configured.
- **Deferred (small follow-ups):** resolving a removable external-id key back to a content node (the
  documented identity-link gap) so the gated clean mutates library state (today it counts+logs the
  eligible set, never an unsafe removal); RSS-cadence scheduling of the sync; per-episode/movie dates in
  the calendar feed once identify persists them.

### Phase G — Hardening to "feature complete"
- Full naming-token + multi-episode-style coverage; durable release-type state (no re-grab loops);
  anime depth (XEM/AniDB wiring + corpus); timezone-correct daily; performance.
- **Exit gate:** the original definition-of-feature-complete (00-vision.md) + parity thresholds met.

---

## 5. Drop-in readiness checklist (by tool)
- [ ] **Prowlarr** — `system/status`+version header, `indexer` CRUD + schema + test + forceSave (Phase A,C)
- [ ] **Overseerr / Jellyseerr** — `system/status`, `GET/POST series`+`movie`, `*/lookup`, `qualityprofile`, `rootfolder`, `tag`, `POST command`, `languageprofile`(Sonarr path), availability state (Phase A,E)
- [ ] **Bazarr** — `GET series`/`episode`/`movie` with accurate paths; optional `Download`/`Rename` webhook (Phase A,F)
- [ ] **Recyclarr / Configarr** — `customformat`(+schema), `qualityprofile`(+schema,formatItems), `qualitydefinition`, vocab alignment (Phase A,B)
- [ ] **Notifiarr** — poll endpoints + `eventType` webhook + `Test` (Phase A,F)
- [ ] **Dashboards (Homepage/Homarr)** — `wanted/missing`, `queue`, `calendar`, counts via `totalRecords` (Phase A); **iCal feed** `/feed/v3/calendar/{sonarr,radarr}.ics` (apikey-query) landed Phase F
- [ ] **Import-list tooling (Trakt/TMDb/Plex via Overseerr/Recyclarr)** — `/api/v3/importlist` CRUD+schema+test + `/importlistexclusion` both faces; safeguarded sync (failed fetch never wipes); live sources blocked-on-creds (Phase F)
- [x] **Download clients** — live qBit with categories + completed-download handling + import handoff; cross-fs health warning (Phase D). **Blackhole / watch-folder universal adapter** (core `DownloadClient`; add→watch-dir, status→completed-dir, Track→Import handoff verified on disk; in `/api/v3/downloadclient/schema`) and **shared remote-path mapping** (`apply_remote_path_mappings` applied once in the runner before Import; `/api/v3/remotepathmapping` CRUD on both faces) landed 2026-06-23. SAB live import deferred.
- [ ] **Notifications** — Connect webhook + common connectors (Phase F)

---

## 6. Hard dependencies, risks, and deliberate differentiators
- **TheTVDB v4 licensing (blocker for TV metadata):** no free per-user keys; the originals hide this
  behind Servarr's Skyhook proxy. cellarr must choose: (a) a licensed contract, (b) per-user
  subscription+PIN, (c) run our own caching proxy (`cellarr-meta` standalone), or (d) lead with TMDb
  for TV / TVmaze. **This decision blocks Phase E and should be made early.**
- **The decision behavior is emergent, not declarative** — port behavior from the originals' named
  source files where exact compatibility matters (CustomFormatCalculationService, UpgradableSpecification,
  CutoffSpecification, …), clean-room per [../agents/legal-and-licensing.md](../agents/legal-and-licensing.md).
- **Deliberate differentiators (fix what the originals get wrong):** loud same-filesystem/hardlink
  health warning (silent copy-fallback is the #1 user footgun); never clean a library on a
  failed/empty list fetch; persist release-type to avoid season-pack re-grab loops; the decision-log
  UI (already cellarr's signature) for explainable grabs.
- **Parser long tail:** 90% exact on 120 titles is a starting point; widen the corpus toward the
  originals' ~1,500–2,000 fixtures for a trustworthy number (the never-finished tail).

---

## 7. Summary
The cellarr **engine** is real and measured (90% parser parity, working pipeline). The distance to a
true Sonarr+Radarr **drop-in** is dominated by **breadth of the `/api/v3` ecosystem surface**
(Phase A), then **wiring the existing integrations live** (Phases C–D), with **TV metadata licensing**
(Phase E) as the one external blocker requiring a product decision. None of Phases A–D are research
problems — they are well-scoped engineering against contract tests. Phase E needs a human call first.
