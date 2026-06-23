# Spec: cellarr-download

## Responsibility
Hand grabs to download clients and track them to completion. Implements the `DownloadClient` trait
for **qBittorrent, Deluge, Transmission** (torrent) and **SABnzbd, NZBGet** (Usenet) for v1; others
follow the same trait. Plus the **blackhole / watch-folder** adapter — the *universal* client that
works with any client at all — and a **shared remote-path-mapping** layer applied once before Import.

## Architecture note: generic where it's genuine, per-client where it isn't
There is no unifying client protocol like Torznab/Cardigann across download clients, and only ~8
clients matter, so the per-client adapters above are the right shape — a declarative "Cardigann for
clients" would be a false abstraction. The two genuine generic wins are built instead:
1. a **blackhole/watch-folder** adapter that works with *any* client (it speaks no client API), and
2. **remote-path mapping** as a *shared* layer (not duplicated per adapter).

## Allowed dependencies
Internal: `cellarr-core`. External: `reqwest`, `tokio`, `serde`, `thiserror`. JSON-RPC helper for
NZBGet.

## Public interface
- `DownloadClient` impls per client, each providing the uniform lifecycle:
  - `add(release, category) -> download_id`
  - `status(download_id) -> DownloadStatus` (state, progress, on-disk path)
  - `completed()` / event hook where supported
  - `remove(download_id, opts)` (gated by seed ratio/time for torrents)
- Capability/version detection per client.

## Blackhole / watch-folder adapter (the universal client)
`BlackholeClient` implements the same `DownloadClient` trait with **no client API**:
- **`add(grab)`** writes the release into a configured **watch directory** for the user's own client
  to pick up: a magnet URL is written verbatim to `<stem>.magnet`; an `http(s)` `.torrent`/`.nzb` URL
  is fetched (through the same `HttpTransport` seam the API clients use) and its bytes written to
  `<stem>.torrent` / `<stem>.nzb`. Returns a **deterministic download id** = the sanitized release
  title stem (so the file name *is* the id; `status`/`remove` recompute it, no persisted handle).
- **`status(id)`** is *filesystem-derived*: it looks in a configured **completed directory** for an
  item matching the id (a finished `<stem>.<ext>` file or a `<stem>/` folder). Until one appears the
  job is `Downloading`; once present it is `Completed` with `content_path` = that item — exactly what
  Import reads. No client API is polled.
- **`remove(id, delete_data)`** removes the watch artifact; with `delete_data` it also removes the
  completed output. Idempotent (a missing artifact is success).

This works with **any** download client: the user points their torrent client / Usenet tool at the
same watch + completed directories. Configured like any client via `/api/v3/downloadclient` with
implementation `TorrentBlackhole` / `UsenetBlackhole` and fields `watchFolder` / `completedFolder`.

## Remote-path mapping (shared layer, applied before Import)
`RemotePathMapping { host, remote_path, local_path }` (a `Vec` on the runner config) is applied in
**one shared place** — the jobs runner, right after Track reads the client-reported `content_path`
and before `plan_import`. So `/downloads/x` as the client sees it becomes `/data/downloads/x` as
cellarr sees it, for *every* download client (the rewrite is not duplicated per adapter). Matching is
a path-boundary prefix replacement (`/downloads` matches `/downloads/x` and `/downloads`, not
`/downloads-extra`); the first mapping whose `host` (empty = any) and `remote_path` match wins; an
unmapped path passes through unchanged. CRUD lives at `/api/v3/remotepathmapping` (both faces) for
Recyclarr/UoMi and users. See `cellarr_core::{RemotePathMapping, apply_remote_path_mappings}`.

## Behavior
- Always assign cellarr's **category/label** so cellarr only touches its own downloads and
  per-category paths work. (The blackhole has no client-side label; it scopes via its dedicated
  watch/completed directories.)
- Prefer webhooks/events over tight polling where the client supports them.
- **Version- and quirk-aware**, treated as first-class:
  - qBittorrent WebUI API v2 (`/api/v2/`), cookie/`SID` auth, `Referer`/`Origin` handling,
    loopback-only auth bypass; handle the 5.x CSRF/Host-header tightening and the late-2025 `/login`
    success-response change.
  - SABnzbd: `mode=` HTTP API, `apikey=`, `output=json`.
  - NZBGet: JSON-RPC positional params, HTTP Basic.
- Completion detection accounts for Usenet repair/unpack before handing off to Import.

## Test obligations
- **Record/replay** per client for the full lifecycle, including **version-divergent** fixtures
  (especially qBittorrent 5.x auth/login variants). No live clients in CI.
- Category scoping tested (cellarr ignores foreign downloads).
- **Blackhole**: hermetic tempdir tests — `add` writes the job into the watch dir (magnet without a
  network call, `.torrent`/`.nzb` via the fetch seam); simulate the external client by placing a
  finished file/folder in the completed dir; `status` → `Completed` + `content_path`; the jobs
  Track→Import handoff actually imports it. Unknown id → `NotFound`; `remove` is idempotent.
- **Remote-path mapping**: a `status()` `content_path` under a mapped remote prefix is rewritten to
  the local prefix before `plan_import` (import succeeds at the local file); an unmapped path passes
  through unchanged (and, being non-existent locally, holds for review).
- Error/edge handling: auth failure, missing item, partial/failed download → correct pipeline
  failure transitions.

## References
[06-integrations.md](../06-integrations.md), [03-pipeline.md](../03-pipeline.md),
[13-upstream-repos.md](../13-upstream-repos.md).
