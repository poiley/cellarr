# `/api/v3` ecosystem-compatibility parity

The drop-in test: can a user point Prowlarr, Overseerr/Jellyseerr, Bazarr, Notifiarr, Recyclarr and
dashboards at cellarr instead of Sonarr/Radarr with no regression? Measured by probing cellarr's
running daemon vs live Sonarr 4.0.17 / Radarr 6.2.1. Ecosystem endpoint requirements are sourced in
the research consolidated into [REPLACEMENT-ROADMAP.md](REPLACEMENT-ROADMAP.md).

## ✅ Phase A status (2026-06-23)
**Implemented.** cellarr's `/api/v3` is now a real drop-in for **both** Sonarr and Radarr via two
faces (`/sonarr/api/v3`, `/radarr/api/v3`) plus the bare `/api/v3` for cellarr's own UI — the user
adds cellarr twice, as a Sonarr and a Radarr. Shapes were captured from live Sonarr 4.0.17 / Radarr
6.2.1 and pinned by contract tests (`crates/cellarr-api/tests/fixtures/`, `tests/v3_faces.rs`). The
table at the bottom and the per-item notes below are updated to ✅/🟡 to reflect what shipped.

## ⚠️ Measurement caveat that changed the picture (historical)
A status-code probe was **misleading** for cellarr: its static-asset handler served `index.html`
(HTTP **200, `text/html`**) as an SPA fallback for any unmatched path — including unimplemented
`/api/v3/*` routes. So "200" ≠ "implemented." We re-probed by **Content-Type**.

> **Bug B1 — FIXED.** Each v3 mount now owns a 404-JSON fallback, so unknown `/api/v3/*` (and
> `/sonarr|radarr/api/v3/*`) paths return `404 {code,message}`; only non-API paths reach the SPA
> asset fallback. Verified by `unknown_api_path_returns_404_json_not_html`.

## Endpoint coverage (post-Phase A)
**Real (JSON), all three mounts:** `ping`, `system/status` (full fields), `health`, `rootfolder`,
`tag` (CRUD), `qualityprofile` (+`formatItems`,`/schema`), `qualitydefinition`,
`customformat` (CRUD,`/schema`), `indexer` (CRUD,`/schema`,`/test`), `series`/`movie` (GET list +
POST add), `episode` (list), `movie/lookup`, `series/lookup`, `calendar`, `queue`, `history`,
`wanted/missing`, `GET`+`POST command`. Unknown v3 paths → 404 JSON.

## Shape gaps — closed
- **`system/status`** — now returns the full captured field set per face (`branch`, `urlBase`,
  `isDocker`, `databaseType/Version`, `packageVersion`, `migrationVersion`, `osName`, `startTime`,
  `isAdmin`, `mode`, `appName`, `version`, …). Verified key-for-key against the fixtures.
- **`qualityprofile`** — now carries **`formatItems[]`** (CF id→score) and `minUpgradeFormatScore`;
  the Radarr face also carries `language`. `qualityprofile/schema` present.
- **`queue`/`history`/`wanted`** — full envelope `page,pageSize,sortKey,sortDirection,totalRecords,records`.
- **`indexer/schema`** — Torznab + Newznab entries with the round-trip `fields[]`.

## Cross-cutting contract — addressed
- **`X-Application-Version`** — present on every API response per face (Sonarr v4 / Radarr v5).
- **Auth modes** — `X-Api-Key` **and** `?apikey=` both honored when a key is set; open when none.
- **Version identity** — Sonarr face = v4 (so `languageprofile` is correctly absent); Radarr face = v5.
- **Webhook/Connect push** — still native WS/SSE only; the `eventType` webhook contract is Phase F.

## Tiered must-haves vs cellarr status
| Tier | Needed by | Endpoints | cellarr status |
|------|-----------|-----------|----------------|
| 1 | everything | `system/status`(+version header), `/ping`, `health`, both auth modes | ✅ full status field set, `X-Application-Version` per face, `/ping`, `/health`, both auth modes |
| 2 | Overseerr, Bazarr | `GET/POST series`/`movie`, `*/lookup`, `qualityprofile`, `rootfolder`, `tag`, `POST command`, accurate file paths | ✅ `GET /series`(+`/episode`)/`/movie` lists with `path`/`*File.path`/`rootFolderPath`/`monitored`/`hasFile`; rootfolder + tag CRUD; `GET`+`POST command` (🟡 `languageprofile` intentionally omitted — emulating Sonarr **v4**, which dropped it) |
| 3 | Prowlarr | `indexer` CRUD, `indexer/schema`, `indexer/test`, `?forceSave=true` | ✅ indexer create/update/list (+idempotent delete), Torznab+Newznab schema, `POST indexer/test`, `?forceSave=true` honored |
| 4 | Recyclarr, Notifiarr, dashboards | `customformat`(+schema), `qualityprofile`(+schema,**formatItems**), `qualitydefinition`, `queue`/`history`(paging), `calendar`, `wanted/missing` | ✅ customformat CRUD + schema (specs round-trip); qualityprofile **formatItems** + `minUpgradeFormatScore` + `/schema`; qualitydefinition; queue/history/wanted full paging envelope; calendar |

## What shipped in Phase A
- **Two faces + bare mount** (`/sonarr/api/v3`, `/radarr/api/v3`, `/api/v3`); one handler core, face
  changes only `appName`/version + which media type's list resources are exposed.
- **B1 fixed** — unknown v3 paths return 404 JSON, not SPA HTML.
- **`X-Application-Version`** header on every API response (Sonarr v4 / Radarr v5 strings).
- **Both auth modes** (`X-Api-Key` + `?apikey=`) when a key is set; open when none.
- **Full `system/status`** field set per face (matches captured fixture keys).
- **Library lists** `GET /series`(+`/episode`)/`GET /movie` with file/path/monitored fields.
- **`rootfolder`, `tag` CRUD, `health`, `qualitydefinition`, `wanted/missing`, `GET command`.**
- **`qualityprofile` `formatItems[]` + `minUpgradeFormatScore` + `/schema`**;
  **`customformat` CRUD + `/schema`** with round-tripping `specifications[]`.
- **`indexer` CRUD + `/schema` (Torznab+Newznab) + `/test` + `?forceSave=true`.**
- **Full paging envelope** on queue/history/wanted.

### Additive core change
`ContentRepository::roots(library)` was added (db + trait) so the library list endpoints can list
series/movie root nodes — `monitored_missing` deliberately excludes container roots (series/season).

## Deliberately deferred (not Phase A)
- **`languageprofile`** — Sonarr v3-only; we emulate v4, where it is gone (Jellyseerr branches on the
  version and skips it). Re-add only if a tool insists on a v3 identity.
- **Title text in list/lookup resources** — the FTS index has no reverse lookup, so list resources
  fall back to the node id for `title`; a title column on the resolved-identity row closes this.
- **`customformat`/`indexer` hard delete** — the persistence layer has no delete yet; the shim
  accepts deletes idempotently (200). Wiring real deletes is a small additive db change.
- **Live `indexer/test` connectivity** and CF **score** sync semantics — Phases B/C (the shim's CF
  score lives on the cellarr `CustomFormat`; `formatItems` surfaces it, but Recyclarr-driven score
  *writes* land with later phases).

## Summary
cellarr's `/api/v3` is now the ~25-endpoint drop-in surface across both faces, contract-tested
against live-app fixtures. Remaining ecosystem work is wiring the existing integrations live
(Phases B–D) and the metadata licensing fork (Phase E), per
[REPLACEMENT-ROADMAP.md](REPLACEMENT-ROADMAP.md).
