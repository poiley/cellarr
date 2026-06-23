# Spec: cellarr-api

## Responsibility
Serve three surfaces from the one binary: the **native REST/WS API**, the **`/api/v3`
Radarr/Sonarr-compatibility shim**, and the **static SRCL frontend** assets.

## Allowed dependencies
Internal: `cellarr-core`, `cellarr-db` (via repo traits), `cellarr-jobs` (commands). External: `axum`,
`tokio`, `tower`/`tower-http`, `serde`, `rust-embed` (frontend assets), an OpenAPI generator,
`tracing`, `thiserror`.

## Public interface
- Native REST under `/api/v1`: libraries, content, files, indexers, clients, profiles,
  custom-formats, decision-log, history, queue, system status, commands.
- **WebSocket/SSE** push for live updates (queue progress, import events, decision-log entries).
- **`/api/v3`** shim mapping the originals' request/response shapes onto cellarr's domain, presenting
  the right app's surface per library type (movies→Radarr-like, TV→Sonarr-like).
- Generated **OpenAPI** spec consumed by the frontend and external clients.
- Auth (API key + optional UI session); structured error bodies (stable `code` + `message`).

## Behavior
- **Do not break `/api/v3`** — it's an external contract that the ecosystem depends on (non-negotiable).
- Push events fire on the correct domain transitions, not on a polling timer.
- Secrets never logged; auth enforced on all mutating endpoints.

## Test obligations
- Handler tests: request → response, auth, error shapes for the native API.
- **`/api/v3` contract tests** against recorded request/response pairs captured from real ecosystem
  clients (Overseerr/Jellyseerr etc.) — this is what proves "existing tools just work."
- A WebSocket test asserting push on real domain transitions.
- OpenAPI spec generation is validated in CI.

## References
[09-api.md](../09-api.md), [10-ui.md](../10-ui.md), [11-testing.md](../11-testing.md).
