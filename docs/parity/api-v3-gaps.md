# `/api/v3` ecosystem-compatibility parity

The drop-in test: can a user point Prowlarr, Overseerr/Jellyseerr, Bazarr, Notifiarr, Recyclarr and
dashboards at cellarr instead of Sonarr/Radarr with no regression? Measured by probing cellarr's
running daemon vs live Sonarr 4.0.17 / Radarr 6.2.1. Ecosystem endpoint requirements are sourced in
the research consolidated into [REPLACEMENT-ROADMAP.md](REPLACEMENT-ROADMAP.md).

## ⚠️ Measurement caveat that changed the picture
A status-code probe is **misleading** for cellarr: its static-asset handler serves `index.html`
(HTTP **200, `text/html`**) as an SPA fallback for any unmatched path — including unimplemented
`/api/v3/*` routes. So "200" ≠ "implemented." We re-probed by **Content-Type**: `application/json`
= real endpoint; `text/html` = SPA fallback = **not implemented**.

> **Bug B1 (real):** the SPA fallback intercepts unknown `/api/v3/*` paths and returns HTML 200.
> Ecosystem tools will try to parse HTML as JSON and misbehave. The asset fallback must be scoped to
> non-API paths; unknown `/api/v3/*` must return **404 JSON**, not the UI.

## Endpoint coverage (by content-type, this run)
**Real (JSON) in cellarr's shim today:** `system/status`, `qualityprofile`, `queue`, `history`,
`calendar`, plus (from the router) `movie/lookup`, `series/lookup`, `POST movie`, `POST series`,
`POST command`.

**NOT implemented (SPA-fallback HTML):**
`health`, `qualitydefinition`, `rootfolder`, `tag`, `languageprofile`, `customformat`(+`/schema`),
`indexer`(+`/schema`), `downloadclient`, `wanted/missing`, `qualityprofile/schema`.

**Implemented for the wrong methods:** `GET /series` and `GET /movie` return **405** (the shim has
`POST` add but not the **GET list** the whole ecosystem reads).

## Shape gaps on endpoints that DO exist
- **`system/status`** — cellarr returns 10 keys; Sonarr returns ~30. Missing keys tools read:
  `branch`, `urlBase`, `isDocker`, `databaseType`, `databaseVersion`, `packageVersion`,
  `migrationVersion`, `osName`, `startTime`, `isAdmin`, `mode`, … Overseerr mainly needs `version` +
  `appName` (present), but be generous for robustness.
- **`qualityprofile`** — missing **`formatItems[]`** (CF id→score) and `minUpgradeFormatScore`.
  **Recyclarr/Configarr cannot sync custom-format scores without `formatItems`.**
- **`queue`** paging envelope — has `page,pageSize,records,totalRecords`; missing
  `sortKey,sortDirection` (minor; dashboards read `totalRecords`).
- **`indexer/schema`** — empty/absent. **Prowlarr round-trips its pushed indexer through the schema;
  empty schema breaks indexer sync.**

## Cross-cutting contract gaps
- **`X-Application-Version` response header — ABSENT.** Prowlarr reads the *header* (not the body) and
  enforces a minimum-version floor; missing header = **rejected**. MUST-HAVE for Prowlarr.
- **Auth modes** — cellarr's demo runs open (no key), so honoring `X-Api-Key` **and** `?apikey=` when a
  key is set is unverified; both are required (Overseerr/Homepage use `?apikey=`).
- **Version identity** — decide which app/version cellarr emulates (Sonarr v4 / Radarr v5) so tools
  land in their "supported" band and take the right code path (e.g. `languageprofile` is Sonarr-v3-only;
  Jellyseerr branches on it).
- **Webhook/Connect push** — `eventType`-discriminated webhook (`Grab`/`Download`/`Rename`/`Health`/
  `Test`) is what Bazarr-push, Notifiarr, and notifications consume. Native cellarr has WS/SSE push but
  not the *arr webhook contract. (Coverage TBD — tracked in the roadmap.)

## Tiered must-haves vs cellarr status
| Tier | Needed by | Endpoints | cellarr status |
|------|-----------|-----------|----------------|
| 1 | everything | `system/status`(+version header), `/ping`, `health`, both auth modes | partial (status thin, **no version header**, health missing) |
| 2 | Overseerr, Bazarr | `GET/POST series`/`movie`, `*/lookup`, `qualityprofile`, `rootfolder`, `tag`, `languageprofile`(Sonarr), `POST command`, accurate file paths | partial (**GET list missing**, rootfolder/tag/languageprofile missing) |
| 3 | Prowlarr | `indexer` CRUD, `indexer/schema`, `indexer/test`, `?forceSave=true` | **missing** |
| 4 | Recyclarr, Notifiarr, dashboards | `customformat`(+schema), `qualityprofile`(+schema,**formatItems**), `qualitydefinition`, `queue`/`history`(paging), `calendar`, `wanted/missing` | partial (queue/history/calendar real; **customformat/qualitydefinition/wanted missing; formatItems missing**) |

## Summary
cellarr's `/api/v3` is a **thin slice** today (~5 real GET endpoints + a few POST/lookup), not the
~25 a drop-in needs. None of this is hard — it's mostly mapping existing cellarr domain data through
more handlers — but it is the **largest single block of work** between "works for cellarr's own UI"
and "drop-in for the ecosystem." Sequenced in [REPLACEMENT-ROADMAP.md](REPLACEMENT-ROADMAP.md).
