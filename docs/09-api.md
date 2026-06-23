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

## The `/api/v3` compatibility shim

A separate router mounted at `/api/v3` that maps the originals' request/response shapes onto
cellarr's domain. Because cellarr is unified, the shim must present the *right* app's surface based
on context (a movies library answers like Radarr; a TV library answers like Sonarr). Scope for v1:
the endpoints the major ecosystem tools actually call (system status, quality profiles, lookup,
add, command, calendar, queue, history). The shim is **contract-tested against recorded
request/response pairs** captured from the real apps and the real ecosystem clients.

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
