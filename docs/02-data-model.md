# 02 — The unified data model

This is the doc that makes "one app for all media types" real or fake. Read it carefully; changes
here ripple through every crate. **Do not modify the model without agreement** (see the agent guide).

## The core tension

The media types do not share a shape:

| Type | Hierarchy | Grab granularity | File ↔ content |
|------|-----------|------------------|----------------|
| Movie | flat (Movie) | the movie | ~1:1 |
| TV | Series → Season → Episode | episode / season / series | one file can satisfy **many** episodes (multi-ep) |
| Music | Artist → Album → Track | album (usually) | one file per track; albums are multi-file |
| Book | Author → Book | book | ~1:1, plus audiobook/ebook editions |

Two failure modes to avoid:
- **A mushy god-table** with 80 nullable columns that means nothing specific.
- **Four parallel schemas** — which is just re-forking the *arr stack in a new language.

## The resolution: generic *structure*, typed *identity*

> **Structure is generic. Identity/metadata is typed. The pipeline only ever touches structure.**

Everything the pipeline needs — monitoring, files, grabs, history, scoring, decisions — is
**generic** and operates on a `ContentRef`. Everything type-specific — TMDb id, runtime, track
length, ISBN, overview text — is **typed** and lives in per-type metadata, off the pipeline's path.

### Generic structural entities (media-type-agnostic)

These tables/types are the same for every media type. (Column lists are illustrative, not final;
the authoritative schema lives in `cellarr-db` migrations — see [08-database.md](08-database.md).)

- **`library`** — a typed collection: `{ id, media_type, name, root_folders[], default_quality_profile }`.
  A user can have several libraries of the same type (e.g. "Movies" and "Movies — 4K").
- **`content`** — the structural tree, an adjacency list. Every monitorable / grabbable /
  file-bearing node is a row: `{ id, library_id, media_type, parent_id, kind, coords, monitored,
  title_id }`. `kind` ∈ {series, season, episode, movie, artist, album, track, author, book}.
  `coords` is the numbering (below). `title_id` links to the typed identity row. Modeled in
  `cellarr-core` as `ContentNode` (the persisted row) with a `ContentKind` discriminator; the
  pipeline carries the slim `ContentRef` view (`ContentNode::as_ref`). `ContentRepository` owns the
  reads/writes: `get` + `monitored_missing` (pipeline reads), plus `upsert` (write a node, parent
  links included) and `children` (walk one level of the adjacency list) so `db`/`media` can build the
  tree.
- **`media_file`** — a physical file: `{ id, path, size, quality, languages, media_info,
  custom_format_score }`. Modeled as `MediaFile` in `cellarr-core`; `quality` is the resolved
  `Quality { name, rank }` (see [05-decision-engine.md](05-decision-engine.md)). Reads/writes live on
  a dedicated **`MediaFileRepository`** (`create` / `get` / `list_for_content` / `delete`) — kept a
  separate aggregate from `ContentRepository` because one file can satisfy several content nodes.
- **`content_file`** — the many-to-many link between `content` and `media_file` (this is how one
  multi-episode file satisfies several episode nodes; the originals special-case this — we model it).
  `MediaFileRepository::list_for_content` resolves through this link.
- **`grab`** — a release we sent to a download client: `{ id, request, download_id, status }`.
  Modeled as `Grab` in `cellarr-core` (wrapping the immutable `GrabRequest` with mutable lifecycle).
  `status` is a `GrabStatus` ∈ {pending, sent, downloading, completed, imported, failed,
  blocklisted}. `GrabRepository` adds `set_download_id` and `set_status` to advance it.
- **`history`** — the immutable event stream of what happened to each content node (grabbed,
  imported, upgraded, deleted, failed).
- **`decision_log`** — *why* the system did what it did (see [03-pipeline.md](03-pipeline.md)).
- **`quality_profile`**, **`custom_format`** — see [05-decision-engine.md](05-decision-engine.md).
- **`indexer`**, **`download_client`**, **`root_folder`**, **`notification`** — configuration.
  Modeled in `cellarr-core::config` as `IndexerConfig`, `DownloadClientConfig`, `RootFolder`, and
  `NotificationConfig`. Each carries the small set of fields the system reasons about generically
  (`id`, `name`, `kind`, `enabled`, and `priority`/`protocol`/`category` as relevant) plus a
  `settings: serde_json::Value` for the adapter-specific bits (API keys, hosts, webhook URLs) — typed
  where the shape is shared, JSON only for the open-ended remainder, per the decision below.

### Typed identity/metadata (per media type)

The rich, type-specific data lives behind the `title_id` reference, in per-type tables:
`movie_meta`, `series_meta`, `season_meta`, `episode_meta`, `artist_meta`, `album_meta`,
`track_meta`, `author_meta`, `book_meta`. These hold external IDs (tmdb/tvdb/imdb/musicbrainz/
isbn/anidb), titles, overviews, runtimes, air dates, etc.

> **Decision:** start with **typed side-tables** (real foreign keys, clean queries, easy
> migrations). Reach for a validated JSON column only where the long tail is genuinely open-ended
> (e.g. source-specific extras). Do not start with a JSON blob "to be flexible" — it pushes
> validation into every reader and defeats the point.

## The numbering abstraction: `Coordinates`

Where the types actually differ is *how you address a unit*. This is the one place the difference
is unavoidable, so we name it explicitly and make it a closed enum (stored as tagged JSON in the
`content.coords` column):

- **Movie** — no coordinates (the movie is the unit).
- **Episode** — `{ season, episode, absolute? }`. The canonical TV addressing. `absolute` is the
  anime absolute episode number; reconciling absolute ↔ season/episode is the swampiest correctness
  problem in the project and is handled via scene mappings (see [04-parser.md](04-parser.md) and
  [07-metadata-service.md](07-metadata-service.md)).
- **Daily** — `{ date }` (ISO `yyyy-mm-dd`). A date-addressed broadcast (a daily show). The `date`
  is a plain string so core needs no calendar dependency.
- **SeasonPack** — `{ season }`. A whole-season release.
- **Absolute** — `{ number }`. An anime absolute episode number on its own.
- **Track** — `{ disc, track }`.
- **Book** — `{ series_position? }`.

**Which stage produces which.** The parser may emit the *advertised* numbering it sees: `Movie`,
`Episode`, `Track`, `Book`, **and** the transient TV variants `Daily`, `SeasonPack`, and `Absolute`.
**Identify** then normalizes the transient ones to canonical addressing — `Daily` → `Episode` via the
series' air-date table, `SeasonPack` → one `Episode` node per covered episode, and `Absolute` →
`Episode { season, episode, absolute: Some(n) }` via the scene mapping. The pipeline downstream of
Identify carries only the canonical variants. The stage that produces each variant is documented on
the `Coordinates` enum in `cellarr-core`.

## `ContentRef` — the pipeline's currency

The pipeline passes around a small handle, never the whole rich object:

```
ContentRef { id, library_id, media_type, coords }
```

Anything beyond this (what's the show's name? what are good search terms? how do we name the
file?) is obtained by asking the **`MediaModule`** for that media type. The pipeline never
branches on `media_type` itself — it delegates. This is the single most important rule that keeps
"one app" from rotting into "four apps in a trench coat."

## The `MediaModule` trait (conceptual)

Each media type implements one trait (lives in `cellarr-media`, defined in `cellarr-core`). It
provides, given a `ContentRef` and its metadata:

- **search terms** — how to query indexers for this unit (titles, aliases, IDs, season/ep params).
- **match** — given a parsed release, which content node(s) it satisfies and with what confidence.
- **naming** — the tokens used by the rename engine to lay the file out on disk.
- **metadata source** — which `MetadataSource` to refresh identity from.

Adding music = implement this trait + a `MetadataSource` for MusicBrainz. It does **not** mean
touching the parser core, the decision engine, the pipeline, the download layer, or the API. The
spec [`specs/cellarr-media.md`](specs/cellarr-media.md) defines the exact interface and its tests.

## Worked examples (sanity check the model)

- **A multi-episode file** `Show.S01E01E02.mkv`: parser yields `Coordinates::Episode` ×2; identify
  resolves two `content` (episode) nodes; one `media_file` row; two `content_file` links. Upgrade
  logic and "do I have this?" queries traverse `content_file`, so the shared file is handled
  correctly everywhere without special cases.
- **An anime release** `[Group] Show - 1071 [1080p]`: parser yields `absolute = 1071`; identify
  uses the scene mapping to convert to `{season, episode}` for this series; everything downstream
  is normal.
- **An album grab** in music: one `grab` at album granularity; on import, each track file links to
  its `track` content node; album "completeness" is a query over child tracks.

If a proposed feature can't be expressed cleanly in this model, that is a signal the model needs a
deliberate change — raise it, don't bolt on a special case.
