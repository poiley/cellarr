# 00 — Vision & scope

## What cellarr is

cellarr is a single self-hosted daemon that automates building and maintaining a media library.
For each thing you want (a movie, a TV series, a music artist/album, a book/author), it:

1. **Monitors** it and knows what you have versus what you want, at what quality.
2. **Searches** indexers for releases that satisfy the gap.
3. **Parses** each release's name into structured facts and **identifies** which item it is.
4. **Decides** whether a release is wanted, an upgrade, or junk — using quality profiles and
   custom-format scoring — and records *why*.
5. **Grabs** the chosen release via a download client and **tracks** it to completion.
6. **Imports** the finished files: parses the actual file, verifies the match, then atomically
   moves/hardlinks and renames them into the library, replacing inferior copies safely.
7. **Notifies** and exposes everything over an API and a terminal-aesthetic web UI.

This is exactly what Radarr/Sonarr/Lidarr/Readarr do — but as one app, with one core, one data
model, one parser, one decision engine, and one integration layer, parameterized by media type.

## The four pillars

1. **One daemon, all media types.** See [01-architecture.md](01-architecture.md) and
   [02-data-model.md](02-data-model.md). Media types are modules behind a `MediaModule` trait.
2. **Stand on the shoulders of giants.** See [13-upstream-repos.md](13-upstream-repos.md). We
   mine upstream *test fixtures* into a neutral corpus, treat upstream behavior as an executable
   spec via a *differential oracle*, and reuse community data (Cardigann definitions, TRaSH
   scores, XEM/anime-list mappings).
3. **Tests are the contract.** See [11-testing.md](11-testing.md). Correctness is a measured,
   monotonically-improving number, not a vibe.
4. **Sacred/SRCL UI only.** See [10-ui.md](10-ui.md).

## Non-negotiables

Repeated from the README and agent guide because they are the spine of every decision:

- One static binary, one container, zero required external services (SQLite default).
- Works fully offline except for job-inherent network calls; no required cloud LLM/SaaS.
- Never corrupt the user's library (stage→verify→commit→log for all destructive ops).
- Ecosystem compatible (`/api/v3` shim).
- UI exclusively from Sacred/SRCL.

## In scope (v1 target)

- **Movies** and **TV** end to end (the two highest-value, highest-complexity types).
- Torznab/Newznab indexers, plus the **Cardigann YAML engine** to consume existing definitions.
- The major download clients: qBittorrent, Deluge, Transmission, SABnzbd, NZBGet.
- Quality profiles + custom formats with TRaSH-compatible scoring semantics.
- A self-hostable metadata service for TMDb (movies) and TheTVDB (TV), with caching.
- The web UI (Sacred) and the REST/WS API + `/api/v3` compatibility shim.
- Migration importers from existing Radarr and Sonarr SQLite databases.

## In scope (post-v1, designed-for now)

- **Music** (MusicBrainz) and **books** (OpenLibrary) modules — the data model and pipeline must
  accommodate them from day one even though they ship later.
- WASM Component Model plugin host for third-party indexers/clients/notifiers/metadata.
- LLM parser fallback (local-first) for the long tail of unparseable release names.
- Postgres backend option for power users.

## Explicitly out of scope

- Being a media *server* (playback/streaming). cellarr acquires and organizes; Jellyfin/Plex/etc.
  play. cellarr should integrate with them as consumers, not replace them.
- A bespoke design system. The UI is Sacred, full stop.
- A torrent/usenet client. We integrate with download clients; we are not one.

## Definition of "feature complete"

cellarr is feature complete when a current Radarr+Sonarr+Prowlarr user can migrate their library
and config, point their existing ecosystem tools at cellarr's `/api/v3`, and experience no
regression in parsing accuracy, decision quality, or integration coverage — while gaining a
unified app, an observable decision log, and a faster core. Parsing/decision parity is measured
against the differential oracle ([11-testing.md](11-testing.md)); anything below the agreed parity
threshold is not done.
