# 15 — Glossary

Domain terms used throughout the docs. Read this when something is unfamiliar.

- **\*arr / the originals** — Radarr (movies), Sonarr (TV), Lidarr (music), Readarr (books),
  Prowlarr (indexers). Shared NzbDrone/.NET lineage under the Servarr org. cellarr's reference apps.
- **Release** — a specific published copy of media (a torrent/NZB) with a name like
  `Show.S02E15.1080p.BluRay.x264-GROUP`. The parser's input.
- **Release name parsing** — turning that string into structured facts (quality, source, group,
  numbering…). See [04-parser.md](04-parser.md).
- **Indexer** — a search service for releases, queried via Torznab (torrents) / Newznab (Usenet).
- **Torznab / Newznab** — the indexer query APIs (HTTP → RSS/XML). `t=caps` describes capabilities.
- **Cardigann** — a declarative YAML schema describing how to scrape a tracker; interpreted by a
  generic engine. ~500+ community definitions exist. See [06-integrations.md](06-integrations.md).
- **Download client** — qBittorrent/Deluge/Transmission/SABnzbd/NZBGet etc. that actually downloads.
- **Category / label** — a tag cellarr assigns to its downloads so it only touches its own.
- **Quality profile** — the user's allowed qualities, ordering, cutoff, and CF-score thresholds.
- **Custom format (CF)** — a named bundle of conditions with a score; releases score by summing
  matching CFs. See [05-decision-engine.md](05-decision-engine.md).
- **TRaSH Guides** — community-maintained CF definitions + recommended scores; cellarr imports them.
- **Cutoff / upgrade-until** — the quality (and CF score) at which cellarr stops upgrading an item.
- **Proper / Repack** — re-released fixes of a flawed earlier release; ranked specially.
- **Metadata source** — TMDb/TheTVDB/MusicBrainz/OpenLibrary/AniDB; identity + descriptive data.
- **Skyhook** — Sonarr's metadata proxy (`skyhook.sonarr.tv`); cellarr rebuilds the *role* as
  `cellarr-meta`. See [07-metadata-service.md](07-metadata-service.md).
- **TheXEM / anime-lists** — community mapping data reconciling scene/TVDB/AniDB episode numbering.
- **Absolute numbering** — anime episode numbering (`Show - 1071`) that must map to season/episode.
- **`ContentRef`** — the small handle the pipeline carries instead of a full media object.
- **`Coordinates`** — the numbering enum (Movie / Episode{season,episode,absolute} / Track / Book).
- **`MediaModule`** — the per-media-type trait providing search/match/naming/metadata behavior.
- **`MetadataSource`** — the trait each metadata adapter implements.
- **The pipeline** — Discover→Parse→Identify→Decide→Grab→Track→Import→Rename→Notify. See
  [03-pipeline.md](03-pipeline.md).
- **Decision log** — the table recording *why* each grab/reject/upgrade happened. A signature feature.
- **stage→verify→commit→log** — the mandatory discipline for any destructive file operation.
- **Writer-actor** — the single task all DB writes funnel through (SQLite single-writer). See
  [08-database.md](08-database.md).
- **Differential oracle** — the harness that diffs cellarr's output against the real *arr apps to
  measure parity. See [11-testing.md](11-testing.md).
- **Corpus** — language-neutral test vectors (parse/scoring/naming/anime) mined from upstream. The
  executable spec for the domain-knowledge components.
- **Clean-room** — reimplementing from behavior/specs/data, not by transcribing source, to avoid
  creating a derivative work. See [agents/legal-and-licensing.md](agents/legal-and-licensing.md).
- **SRCL / Sacred** — the MIT terminal-aesthetic React component library; the *only* UI source.
- **Simulacrum** — SRCL's zero-dependency CLI framework (mirrors components in the terminal); a
  candidate for a future cellarr TUI.
- **WASM Component Model / WIT** — the sandboxed plugin technology (`wasmtime`) for third-party
  integrations. See [06-integrations.md](06-integrations.md).
