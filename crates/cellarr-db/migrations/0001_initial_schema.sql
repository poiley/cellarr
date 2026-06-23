-- cellarr initial schema.
--
-- The authoritative schema lives here (migrations are the source of truth; the
-- docs describe the model). Design follows docs/02-data-model.md: the structural
-- entities are generic across media types and the pipeline only ever touches
-- them; the rich, type-specific identity/metadata lives in typed side-tables
-- behind `content.title_id`.
--
-- Portability note: identifiers are stored as TEXT (UUID strings) and timestamps
-- as TEXT (RFC3339) so the same SQL serves both SQLite and Postgres without the
-- sqlx `uuid`/`time` type-mapping features. Tagged-enum / structural values that
-- core already serializes as JSON (coords, release, decision, …) are stored as
-- TEXT holding that JSON.

-- ---------------------------------------------------------------------------
-- Generic structural entities
-- ---------------------------------------------------------------------------

-- A typed collection of content of a single media type.
CREATE TABLE library (
    id                      TEXT PRIMARY KEY NOT NULL,
    media_type              TEXT NOT NULL,
    name                    TEXT NOT NULL,
    -- JSON array of root-folder paths (the model carries several per library).
    root_folders            TEXT NOT NULL DEFAULT '[]',
    default_quality_profile TEXT NOT NULL
);

-- The structural tree as an adjacency list. Every monitorable / grabbable /
-- file-bearing node is a row. `coords` holds the tagged-JSON Coordinates value.
CREATE TABLE content (
    id          TEXT PRIMARY KEY NOT NULL,
    library_id  TEXT NOT NULL REFERENCES library(id) ON DELETE CASCADE,
    media_type  TEXT NOT NULL,
    parent_id   TEXT REFERENCES content(id) ON DELETE CASCADE,
    -- One of: series, season, episode, movie, artist, album, track, author, book.
    kind        TEXT NOT NULL,
    -- Tagged-JSON Coordinates (e.g. {"type":"episode","season":1,"episode":2}).
    coords      TEXT NOT NULL,
    monitored   INTEGER NOT NULL DEFAULT 1,
    -- Link to the typed identity row (movie_meta/series_meta/...). Untyped FK on
    -- purpose: which table it points at depends on `kind`/`media_type`.
    title_id    TEXT
);
CREATE INDEX idx_content_library ON content(library_id);
CREATE INDEX idx_content_parent ON content(parent_id);
CREATE INDEX idx_content_monitored ON content(monitored);
CREATE INDEX idx_content_title ON content(title_id);

-- A physical media file on disk.
CREATE TABLE media_file (
    id          TEXT PRIMARY KEY NOT NULL,
    path        TEXT NOT NULL,
    size        INTEGER NOT NULL DEFAULT 0,
    -- JSON array of language codes.
    languages   TEXT NOT NULL DEFAULT '[]',
    -- JSON: the parsed quality (resolution/source/etc.) for this file.
    quality     TEXT,
    -- JSON: extracted media-info (codecs, runtime, …); open-ended, so JSON.
    media_info  TEXT,
    quality_rank INTEGER,
    custom_format_score INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX idx_media_file_path ON media_file(path);

-- Many-to-many link between content and media_file. This is how one
-- multi-episode file satisfies several episode nodes.
CREATE TABLE content_file (
    content_id    TEXT NOT NULL REFERENCES content(id) ON DELETE CASCADE,
    media_file_id TEXT NOT NULL REFERENCES media_file(id) ON DELETE CASCADE,
    PRIMARY KEY (content_id, media_file_id)
);
CREATE INDEX idx_content_file_file ON content_file(media_file_id);

-- A release sent to a download client.
CREATE TABLE grab (
    id            TEXT PRIMARY KEY NOT NULL,
    -- JSON ContentRef the grab is intended to satisfy.
    content_ref   TEXT NOT NULL,
    -- JSON Release.
    release       TEXT NOT NULL,
    indexer_id    TEXT NOT NULL,
    client_id     TEXT NOT NULL,
    category      TEXT NOT NULL,
    -- The download client's own id once it accepts the grab.
    download_id   TEXT,
    status        TEXT NOT NULL DEFAULT 'queued',
    created_at    TEXT NOT NULL
);
CREATE INDEX idx_grab_status ON grab(status);
CREATE INDEX idx_grab_download ON grab(download_id);

-- Immutable event stream: what happened to each content node.
CREATE TABLE history (
    id          TEXT PRIMARY KEY NOT NULL,
    at          TEXT NOT NULL,
    content_id  TEXT NOT NULL,
    run_id      TEXT NOT NULL,
    -- JSON HistoryEvent (tagged: grabbed/imported/upgraded/…).
    event       TEXT NOT NULL
);
CREATE INDEX idx_history_content ON history(content_id, at);
CREATE INDEX idx_history_run ON history(run_id);

-- Immutable decision stream: why the system acted, one per transition.
CREATE TABLE decision_log (
    id          TEXT PRIMARY KEY NOT NULL,
    at          TEXT NOT NULL,
    run_id      TEXT NOT NULL,
    -- JSON Transition.
    transition  TEXT NOT NULL,
    -- JSON Decision when the transition reached a verdict, else NULL.
    decision    TEXT,
    note        TEXT
);
CREATE INDEX idx_decision_log_run ON decision_log(run_id, at);

-- ---------------------------------------------------------------------------
-- Configuration / settings
-- ---------------------------------------------------------------------------

CREATE TABLE quality_profile (
    id   TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    -- JSON-serialized QualityProfile (allowed qualities, cutoff, CF thresholds).
    body TEXT NOT NULL
);

CREATE TABLE custom_format (
    id    TEXT PRIMARY KEY NOT NULL,
    name  TEXT NOT NULL,
    score INTEGER NOT NULL DEFAULT 0,
    -- JSON-serialized CustomFormat (conditions + modifiers).
    body  TEXT NOT NULL
);

CREATE TABLE indexer (
    id       TEXT PRIMARY KEY NOT NULL,
    name     TEXT NOT NULL,
    protocol TEXT NOT NULL,
    enabled  INTEGER NOT NULL DEFAULT 1,
    -- JSON settings. Secrets within are stored encrypted at rest and never logged.
    settings TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE download_client (
    id       TEXT PRIMARY KEY NOT NULL,
    name     TEXT NOT NULL,
    protocol TEXT NOT NULL,
    enabled  INTEGER NOT NULL DEFAULT 1,
    -- JSON settings; credentials encrypted at rest and never logged.
    settings TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE root_folder (
    id         TEXT PRIMARY KEY NOT NULL,
    library_id TEXT REFERENCES library(id) ON DELETE SET NULL,
    path       TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_root_folder_path ON root_folder(path);

CREATE TABLE notification (
    id       TEXT PRIMARY KEY NOT NULL,
    name     TEXT NOT NULL,
    kind     TEXT NOT NULL,
    enabled  INTEGER NOT NULL DEFAULT 1,
    -- JSON settings; secrets encrypted at rest and never logged.
    settings TEXT NOT NULL DEFAULT '{}'
);

-- ---------------------------------------------------------------------------
-- Cache (no Redis: in-process moka + this DB table cover caching)
-- ---------------------------------------------------------------------------

CREATE TABLE cache (
    cache_key  TEXT PRIMARY KEY NOT NULL,
    value      TEXT NOT NULL,
    -- RFC3339 expiry; NULL = no expiry.
    expires_at TEXT
);
CREATE INDEX idx_cache_expires ON cache(expires_at);

-- ---------------------------------------------------------------------------
-- Typed identity side-tables (per media type).
--
-- These hold external IDs, titles, overviews, runtimes, air dates, etc. — the
-- rich, type-specific data the pipeline never touches. Movie and TV
-- (series/season/episode) are implemented here. Music (artist/album/track) and
-- book (author/book) metadata tables are intentionally deferred to a later
-- migration: their content nodes already work (structure is generic), and the
-- typed-identity columns can land when the music/book MediaModules do.
-- ---------------------------------------------------------------------------

CREATE TABLE movie_meta (
    title_id   TEXT PRIMARY KEY NOT NULL,
    title      TEXT NOT NULL,
    sort_title TEXT,
    year       INTEGER,
    tmdb_id    INTEGER,
    imdb_id    TEXT,
    overview   TEXT,
    runtime    INTEGER
);
CREATE INDEX idx_movie_meta_tmdb ON movie_meta(tmdb_id);
CREATE INDEX idx_movie_meta_imdb ON movie_meta(imdb_id);

CREATE TABLE series_meta (
    title_id   TEXT PRIMARY KEY NOT NULL,
    title      TEXT NOT NULL,
    sort_title TEXT,
    year       INTEGER,
    tvdb_id    INTEGER,
    tmdb_id    INTEGER,
    imdb_id    TEXT,
    overview   TEXT
);
CREATE INDEX idx_series_meta_tvdb ON series_meta(tvdb_id);

CREATE TABLE season_meta (
    title_id      TEXT PRIMARY KEY NOT NULL,
    series_title_id TEXT REFERENCES series_meta(title_id) ON DELETE CASCADE,
    season_number INTEGER NOT NULL,
    overview      TEXT
);
CREATE INDEX idx_season_meta_series ON season_meta(series_title_id);

CREATE TABLE episode_meta (
    title_id        TEXT PRIMARY KEY NOT NULL,
    series_title_id TEXT REFERENCES series_meta(title_id) ON DELETE CASCADE,
    season_number   INTEGER NOT NULL,
    episode_number  INTEGER NOT NULL,
    absolute_number INTEGER,
    title           TEXT,
    overview        TEXT,
    air_date        TEXT
);
CREATE INDEX idx_episode_meta_series ON episode_meta(series_title_id, season_number, episode_number);
