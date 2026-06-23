# Spec: cellarr-download

## Responsibility
Hand grabs to download clients and track them to completion. Implements the `DownloadClient` trait
for **qBittorrent, Deluge, Transmission** (torrent) and **SABnzbd, NZBGet** (Usenet) for v1; others
follow the same trait.

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

## Behavior
- Always assign cellarr's **category/label** so cellarr only touches its own downloads and
  per-category paths work.
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
- Error/edge handling: auth failure, missing item, partial/failed download → correct pipeline
  failure transitions.

## References
[06-integrations.md](../06-integrations.md), [03-pipeline.md](../03-pipeline.md),
[13-upstream-repos.md](../13-upstream-repos.md).
