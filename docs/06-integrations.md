# 06 — Integrations: indexers, download clients, plugins

Integrations are the "long tail" — individually small, collectively enormous, and the part that
breaks most often because it depends on third parties that change without notice. The strategy:
**maximize reuse of community data, compile in the high-value clients, contract-test everything,
and sandbox third-party extensions in WASM.**

## Indexers

### Protocols: Torznab / Newznab
Indexers are queried over **Newznab** (Usenet) and **Torznab** (torrents) — near-identical HTTP
APIs returning RSS/XML. Key facts the implementation must honor:

- The **`t=caps`** endpoint is mandatory and called first; it returns server limits, retention, the
  supported search modes + params, and the full category tree. **Read caps; never hardcode
  categories or assume a param is supported.**
- Search functions: `caps`, `search`, `tvsearch`, `movie`, `music`, `book`. ID params
  (`tvdbid`/`imdbid`/`tmdbid`/`season`/`ep`) are ANDed. Categories use the thousands-based scheme
  (2000 movies, 3000 audio, 5000 TV, 7000 books, subcats like 5040 TV/HD, 5070 TV/Anime).
- Responses are `<rss><channel><item>` with repeated `<torznab:attr name= value=>` pairs and an
  `<enclosure>` pointing to the `.nzb`/`.torrent`/magnet. Torznab adds seeders/peers/infohash and
  freeleech factors.

Implemented in `cellarr-indexers`. See [`specs/cellarr-indexers.md`](specs/cellarr-indexers.md).

### The Cardigann YAML engine (the big reuse win)
Hundreds of trackers are described declaratively as **Cardigann YAML** definitions (login, search
paths, row/field selectors with CSS/XPath + regex filters + templating). These are **data, not
code**. cellarr ships a **generic engine** that interprets these definitions at runtime and exposes
each as a normal Torznab source. This turns 500+ indexers into a folder of YAML.

- Definition structure (top-level keys): `id`, `name`, `language`, `type`, `encoding`, `links`,
  `caps` (with `categorymappings`, `modes`), `settings`, `login`, `download`, `search`
  (`paths`, `inputs`, `rows`, `fields`).
- **Licensing caveat:** the community definitions repo (`Prowlarr/Indexers`) has **no declared
  license**. Do **not** vendor those YAML files into cellarr's repo. Instead: implement the engine
  (our own code), and let users point cellarr at a definitions source they choose (the same way the
  originals consume an external definitions repo). Document this clearly. See
  [agents/legal-and-licensing.md](agents/legal-and-licensing.md).
- Build the engine against a **corpus of recorded HTTP responses** so it's testable offline without
  hitting live trackers (see Testing below).

### Prowlarr-style centralization (built in)
Because cellarr is one app, the Prowlarr role (configure an indexer once, use it everywhere) is
*internal* — there's no separate app to sync to. Indexers are configured once and available to all
libraries/media types. We may also expose a Torznab/Newznab proxy endpoint for external apps, and
optionally *consume* an existing Prowlarr as an indexer source for migrating users.

## Download clients

Compile in the high-value clients in `cellarr-download`: **qBittorrent, Deluge, Transmission**
(torrent) and **SABnzbd, NZBGet** (Usenet). Others (rTorrent, Transmission-rpc variants) follow the
same trait. The **blackhole / watch-folder** adapter is also built — it is the universal client (see
below).

There is no Torznab/Cardigann-style unifying protocol across download clients, and only a handful of
clients matter, so the per-client adapters are the right design — a declarative "Cardigann for
clients" would be a false abstraction. The two genuine *generic* wins are built explicitly: the
blackhole adapter and a shared remote-path-mapping layer.

### Blackhole / watch-folder (the universal client)
`BlackholeClient` implements the same `DownloadClient` trait with **no client API at all** — it is
the lowest common denominator every torrent client and Usenet tool already supports, a watched
folder. `add` drops the release (`.torrent`/`.nzb` fetched from the grab URL, or a `.magnet` written
verbatim) into a configured **watch folder**; the user's own client — whatever it is, including
clients cellarr ships no adapter for — is pointed at that folder, downloads the job, and drops the
finished content into a configured **completed folder**. `status` is filesystem-derived: `Downloading`
until a matching output appears in the completed folder, then `Completed` with the on-disk
`content_path` Import reads. The download id is deterministic (the sanitized release title). Configure
it like any client via `/api/v3/downloadclient` — implementation `TorrentBlackhole` / `UsenetBlackhole`,
fields `watchFolder` / `completedFolder`.

### Remote-path mapping (shared layer)
When the download client and cellarr see paths differently (different hosts/mounts), a
`RemotePathMapping { host, remote_path, local_path }` rewrites the client-reported `content_path`
(`/downloads/x` → `/data/downloads/x`) **before Import**. It is applied in **one shared place** (the
jobs runner), so the rewrite is never duplicated per adapter and every client — including the
blackhole — benefits. Sonarr/Radarr-compatible CRUD lives at `/api/v3/remotepathmapping` (both faces)
since Recyclarr/UoMi and users expect it.

The uniform lifecycle every adapter implements:
1. **Add** the release with an assigned **category/label** (e.g. `cellarr-tv`, `cellarr-movies`) so
   cellarr only ever touches its own downloads and per-category paths work.
2. **Poll/observe** state, progress, and the on-disk path (prefer webhooks/events where the client
   supports them).
3. **Detect completion** (Usenet: after repair/unpack).
4. Hand off to **Import** ([03-pipeline.md](03-pipeline.md)).
5. **Remove/clean up** per settings, gated by seed ratio/time for torrents.

Client-specific facts the adapters must handle (from research):
- **qBittorrent**: WebUI API v2 under `/api/v2/`; cookie/session auth (`POST /api/v2/auth/login`
  → `SID` cookie, resend on every call); usually needs matching `Referer`/`Origin`; localhost
  auth-bypass only covers loopback, so LAN/container callers must authenticate. The 5.x line
  tightened CSRF/Host-header/Referer validation and a late-2025 dev build changed the `/login`
  success response (which broke the originals' success check). **Treat client auth/version quirks
  as first-class, version-detected behavior, and contract-test them.**
- **SABnzbd**: `mode=`-based HTTP API, `apikey=` auth, `output=json`.
- **NZBGet**: JSON-RPC (positional params), HTTP Basic auth.

See [`specs/cellarr-download.md`](specs/cellarr-download.md).

## Import lists (the "what to want" sources)

An **import list** pulls a curated set of items from an external source and adds
the monitored ones cellarr does not already have — the originals' "Import List" /
"List Sync". The design is a **source abstraction** plus a **safeguarded sync**:

- **`ListSource` trait** (`cellarr-core::importlist`) is the abstraction every
  backend implements (`fetch() -> FetchResult`). The live backends —
  `TraktListSource`, `TmdbListSource`, `PlexWatchlistSource` (in
  `cellarr-jobs::importlists::sources`) — are wired but **blocked-on-key**: with no
  credential in the list's `settings` they return a graceful
  `FetchResult::Failed` (never a network call, never a falsely-empty success). A
  deterministic `MockListSource` exercises the framework offline.
- **`/api/v3/importlist`** CRUD + `/schema` + `/test` and **`/api/v3/importlistexclusion`**
  CRUD live on both faces (Sonarr/Radarr) and the bare cellarr face, so
  Overseerr/Recyclarr-style tooling round-trips lists. Persisted via the
  `import_list` / `import_list_exclusion` tables (JSON body + typed columns, like
  the other config aggregates).

### ⚠️ The empty-vs-failed safeguard (the #1 library-wipe footgun)

The originals' optional *clean-library* action (unmonitor/remove items that fell
off a list) is only safe against a **confirmed-good** fetch. The classic
catastrophe: a source errors (auth expired, tracker down), returns *nothing*, and
a naive sync treats "empty" as "the list is now empty" and **wipes the library**.

cellarr makes that impossible by construction. `FetchResult` is explicitly either
`Fetched(items)` (confirmed-good — an empty `Vec` legitimately means the list is
empty) or `Failed(reason)`. The pure `sync_import_list` then:

- **never** marks anything removable on a `Failed` fetch — not even when the
  failure surfaced as an empty item set (`SyncOutcome::removable` is *always* empty
  unless the fetch was confirmed-good **and** the list opted into a destructive
  `CleanAction`);
- persists `last_successful_sync` **only** on a confirmed-good fetch (stamped via
  `ImportListRepo::mark_synced`), so clean logic can require a recent good sync; and
- defaults `CleanAction` to `None` and maps any unrecognized/absent v3
  `cleanLibraryLevel` to the safe `None` (a destructive action is strictly opt-in).

This is contract-tested: `cellarr-core` unit tests prove the pure diff, and
`cellarr-jobs/tests/import_list_sync.rs` proves it end-to-end against the real db —
a source that errors (or returns empty due to error) **removes/cleans nothing** and
leaves a populated library fully intact, while a real successful list adds its
items. The live credential-gated sources are asserted to fail gracefully.

## iCal / ICS calendar feed

The ecosystem (and Google/Apple Calendar, dashboards) subscribes to a
Sonarr/Radarr **iCal feed** for upcoming/aired episodes and movie release dates.
cellarr serves the same at **`/feed/v3/calendar/sonarr.ics`** (TV) and
**`/feed/v3/calendar/radarr.ics`** (movies), authenticated by the **`apikey` query
parameter** calendar clients append to the URL (they cannot send a header). The
`cellarr-api::calendar` module is a pure, spec-valid ICS writer (RFC 5545: CRLF
line endings, text escaping, 75-octet line folding, all-day `DTSTART;VALUE=DATE`
VEVENTs) plus a feed handler that collects events from the library's dated content
(today: TV daily-coded episodes; per-episode air dates / movie release dates land
when the identify pipeline persists them). An empty feed is still a valid empty
`VCALENDAR`. Tested in `cellarr-api/tests/v3_import_list_calendar.rs`.

## Third-party plugins: WASM Component Model (post-v1)

For integrations we don't ship, `cellarr-plugins` hosts **WASM components** via **`wasmtime`**,
with interfaces defined in **WIT**. This is the modern, safe answer to "no stable Rust plugin ABI":

- **Sandboxed:** no ambient authority — a plugin gets only the capabilities the host explicitly
  grants (e.g. an HTTP-fetch capability, not raw sockets). CPU bounded by epoch interruption + fuel;
  memory bounded by `StoreLimits`; fresh instance per invocation for untrusted code.
- **Language-agnostic:** plugin authors aren't forced into Rust.
- **Targets:** custom indexers, download clients, notifiers, and metadata sources.
- **Posture (2026):** host on wasmtime v46+, default to WASIp2 sync for stability, prefer a narrow
  host-provided HTTP capability over `wasi-sockets`. See [`specs/cellarr-plugins.md`](specs/cellarr-plugins.md).

## Testing integrations (record/replay)

Live third parties are not a test dependency. Every integration is tested against **recorded
fixtures**:

- Indexers/Cardigann: recorded `t=caps` and search HTTP responses → asserted parse into normalized
  releases.
- Download clients: recorded API exchanges for each client *and each known-divergent version* (esp.
  qBittorrent 5.x) → asserted lifecycle behavior.
- Contract tests run in CI offline; a separate, opt-in "live smoke" suite hits real services and is
  never on the critical CI path.

Because protocol churn is structural (a 403, a changed schema), **inference does not save
integrations** — fast contract tests + quick patches do. This is called out so no one designs a
"self-healing" integration layer that can't actually work. See [11-testing.md](11-testing.md).
