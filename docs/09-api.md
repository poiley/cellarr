# 09 — API

`cellarr-api` (axum) serves three surfaces from the one binary:

1. **The native cellarr API** — clean, versioned REST + WebSocket, for the cellarr web UI and new
   integrations.
2. **The `/api/v3` compatibility shim** — emulates the Radarr/Sonarr v3 REST API so the existing
   ecosystem (Overseerr/Jellyseerr, Notifiarr, etc.) works unmodified. **This is an adoption cheat
   code and a non-negotiable.**
3. **Static assets** — the built SRCL frontend ([10-ui.md](10-ui.md)), embedded via `rust-embed`,
   so the single binary serves the UI too.

## Native API

- **REST** for CRUD and commands: libraries, content, files, indexers, download clients, quality
  profiles, custom formats, the decision log, history, system status.
- **WebSocket / SSE push** for live updates (queue progress, import events, decision-log entries).
  The originals poll; cellarr pushes — strictly better UX and lighter on the server.
- **OpenAPI** spec generated from the handlers; the frontend and external clients use it.
- **Auth**: API key + optional form/session auth for the UI. Secrets never logged.
- **Errors**: structured machine-readable error bodies (stable `code` + human `message`), not bare
  HTTP statuses, so clients can branch on `code`.

## Web-UI authentication (single-admin)

Two **independent** auth layers, mirroring Sonarr/Radarr (which exempt the apikey API from the UI
login):

- **`/api/v3` apikey** (`crate::auth`) — unchanged. The `X-Api-Key` header / `?apikey=` query gate
  on mutating `/api/v3` routes (and the calendar feed) is what the *arr ecosystem speaks; it is
  **never** governed by the web-UI gate below.
- **Web-UI gate** (`crate::webauth`) — a single-admin login over the SPA + the UI's private
  `/api/v1` surface. Mirrors Sonarr/Radarr's "Authentication" dropdown minus multi-user: there is
  **one** admin (a username + a password *hash*) and a method of **`none | forms | basic`**
  (`core::AuthMethod`). Persisted as a single-row `auth_config` JSON document (migration `0012`);
  the plaintext password is never modelled or stored — only an **Argon2id PHC hash** (`argon2`),
  never logged.

The gate (an axum layer over the whole router) decides per request:

- **Exempt** (always reachable): `/api/v3` + the `/sonarr|radarr/api/v3` faces + `/feed/*`,
  `POST /login`, `POST /logout`, `GET /health`, and the static assets the login page needs
  (`/_next/*`, `favicon.ico`, and any `.js/.css/.woff2/…` resource). HTML documents are **not**
  exempt, so a gated navigation redirects to the login page.
- **None** → pass through (the zero-config default; a fresh install is open on loopback).
- **Basic** → require `Authorization: Basic base64(user:pass)` matching the admin (constant-time
  username compare + Argon2 verify); else `401` + `WWW-Authenticate: Basic realm="cellarr"`.
- **Forms** → require a valid session cookie; missing/invalid → `401` for an `/api/*`/XHR request, or
  a `303 → /login` redirect for a browser navigation. `POST /login {username,password}` verifies the
  hash and mints a **CSPRNG** session token (256-bit, in `auth_session`) in an **HttpOnly;
  SameSite=Lax; Path=/** cookie (`+Secure` behind TLS is a documented follow-up); `POST /logout`
  deletes the session. Sessions expire after 7 days; a method/credential change revokes all
  sessions.

**Lockout safety:** selecting `forms`/`basic` with **no credential yet** is allowed but **not
effectively enforced** (`core::AuthConfig::is_effectively_enforced`) — the gate passes through so the
operator can finish setup. Admin endpoints (under the gated `/api/v1`, but **not** behind the apikey
middleware): `GET /api/v1/auth/config` (status: method/configured/enforced/username, never the
hash), `PUT /api/v1/auth/config {method}`, `POST /api/v1/auth/credential {username,password}`.

## The `/api/v3` compatibility shim

A router that maps the originals' request/response shapes onto cellarr's domain. Because a real
stack configures a *Sonarr* (TV) and a *Radarr* (movies) **separately** — each a URL + key — and
cellarr is one app, the shim exposes the same handler core under **three mounts (two faces)**:

| Mount | Face | `appName` | Version | List resources |
|-------|------|-----------|---------|----------------|
| `/sonarr/api/v3/*` | **Sonarr face** | `Sonarr` | a current Sonarr v4 string | `series`, `episode` |
| `/radarr/api/v3/*` | **Radarr face** | `Radarr` | a current Radarr v5 string | `movie` |
| `/api/v3/*` | cellarr's own | per addressed library | per surface | `series` + `movie` |

The user **adds cellarr twice**: once as a Sonarr (`…/sonarr`) and once as a Radarr (`…/radarr`),
exactly as they would two separate apps. Only `appName`/version and which media type's list
resources are exposed differ between faces; everything else is one code path.

Phase A implements the ecosystem-core surface (Prowlarr, Overseerr/Jellyseerr, Bazarr, Recyclarr,
Notifiarr, dashboards):

- **`X-Application-Version` response header** on every API response (Prowlarr's min-version floor).
- **Both auth modes** when a key is set: `X-Api-Key` header **and** `?apikey=` query.
- `ping`, `system/status` (full field set), `health`, `rootfolder`, `tag` (CRUD),
  `qualitydefinition`, `wanted/missing`, `GET`+`POST command`.
- `GET /series`(+`/episode`) on the Sonarr face, `GET /movie` on the Radarr face, with
  `path`/`*File.path`/`rootFolderPath`/`monitored`/`hasFile`, plus the existing `POST` add + `/lookup`.
- `qualityprofile` with **`formatItems[]`** (CF id→score) + `minUpgradeFormatScore` +
  `qualityprofile/schema`; `customformat` full CRUD + `customformat/schema` (Recyclarr round-trips
  `specifications[]`).
- `indexer` CRUD + `indexer/schema` (Torznab + Newznab) + `POST indexer/test` + `?forceSave=true`.
- Full paging envelope (`page,pageSize,sortKey,sortDirection,totalRecords,records`) on list endpoints.
- **`system/backup`** — `GET` (list), `POST` (create a manual backup now), `GET {id}` (download the
  bundle), `DELETE {id}`, plus restore: `POST system/backup/restore/{id}` and
  `POST system/backup/restore/upload` (raw bundle body). A backup is a single self-contained file
  (`*.cbk`, a `CELLARRBKP1` container = length-prefixed manifest + a `VACUUM INTO` DB snapshot) under
  `<data_dir>/backups`; a daily scheduled backup runs in the daemon with retention 7. **Restore is
  destructive and fenced:** it takes an automatic *pre-restore safety backup* of the live DB, then
  `PRAGMA integrity_check`-validates the snapshot **before** touching the live file, then atomically
  renames it into place. The running pool keeps the old inode, so the swap takes effect on **restart**
  (`restartRequired: true` in the response). Engine: `cellarr_api::backup`; snapshot: `Database::snapshot_to`.
  Postgres backup/restore is a documented `// TODO` (the SQLite-file swap does not apply).
- **`log/file`** — `GET` (list the daemon's on-disk log files) + `GET {name}` (tail recent lines,
  `?limit=`/`?lines=`, capped). The daemon writes a daily-rolling `cellarr.log` under `<data_dir>/logs`
  (the CLI installs `tracing-appender`). The `{name}` is traversal-guarded (bare log filename only).
- **Expanded `health`** (`cellarr_api::health`): `no-root-folder`/`root-folder-unwritable` (a real
  local write-probe), `no-indexer`, `no-download-client`, `no-recent-backup`, and `database-ok` (a
  liveness probe). Each record carries `{source,type,message,wikiUrl}`; the wikiUrl anchor is the
  check `type` slug. The live `*-unreachable` network probes are a `// TODO` pending a reachability
  seam (skipped, not guessed, to honor the offline non-negotiable).

**Bug B1 (fixed):** unknown `/api/v3/*` (and `/sonarr|radarr/api/v3/*`) paths now return **404 JSON**
(`{code,message}`), never the SPA HTML fallback — the asset fallback is scoped to non-API paths via
a 404-JSON fallback owned by each v3 mount.

The shim is **contract-tested against fixtures captured from live Sonarr 4.0.17 / Radarr 6.2.1**
(`crates/cellarr-api/tests/fixtures/`); the suite diffs cellarr's JSON shapes against them.

> Maintaining the shim is a feature, not tech debt. Breaking it breaks users' existing tools.

## Versioning & stability

- Native API is versioned (`/api/v1`); breaking changes bump the version.
- The `/api/v3` shim tracks the originals' v3 surface and is treated as an external contract.

## Testing

- Handler-level tests for the native API (request → response, auth, error shapes).
- **Contract tests** for the `/api/v3` shim against recorded pairs from real ecosystem clients —
  this is what proves "Overseerr just works."
- A WebSocket test that asserts push events fire on the right domain transitions.

See [`specs/cellarr-api.md`](specs/cellarr-api.md).
