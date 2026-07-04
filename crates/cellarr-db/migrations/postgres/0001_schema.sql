-- cellarr Postgres schema — the end-state of the SQLite migration set, as one
-- consolidated schema.
--
-- The SQLite engine (migrations/sqlite/) is the source of truth and evolves via
-- incremental migrations. Postgres is a newer, opt-in backend with no existing
-- databases in the field, so it does not need that history: this single file is
-- the exact end-state schema, kept in lockstep with the SQLite end-state.
--
-- Type mapping (the repositories decode both backends with one set of
-- `try_get::<T>` calls, so each column's Postgres type must decode to the same
-- Rust type its SQLite twin does):
--   TEXT     -> text            (UUIDs and RFC3339 timestamps are stored as text,
--                                exactly as on SQLite — no uuid/time mapping)
--   INTEGER  -> bigint          (every integer column is read as i64, including
--                                the 0/1 boolean-ish `monitored`/`enabled` flags)
--   REAL     -> double precision (read as f64)
-- Structural JSON values (coords, release, decision, settings bodies, …) are
-- stored as text holding that JSON, identical to SQLite.
--
-- Full-text search: SQLite uses an FTS5 virtual table; Postgres uses a real
-- table with a generated `tsvector` column and a GIN index (see content_fts).

-- ---------------------------------------------------------------------------
-- Generic structural entities
-- ---------------------------------------------------------------------------

CREATE TABLE library (
    id                      text PRIMARY KEY NOT NULL,
    media_type              text NOT NULL,
    name                    text NOT NULL,
    root_folders            text NOT NULL DEFAULT '[]',
    default_quality_profile text NOT NULL
);

CREATE TABLE content (
    id          text PRIMARY KEY NOT NULL,
    library_id  text NOT NULL REFERENCES library(id) ON DELETE CASCADE,
    media_type  text NOT NULL,
    parent_id   text REFERENCES content(id) ON DELETE CASCADE,
    kind        text NOT NULL,
    coords      text NOT NULL,
    monitored   bigint NOT NULL DEFAULT 1,
    title_id    text,
    series_type text NOT NULL DEFAULT 'standard'
);
CREATE INDEX idx_content_library ON content(library_id);
CREATE INDEX idx_content_parent ON content(parent_id);
CREATE INDEX idx_content_monitored ON content(monitored);
CREATE INDEX idx_content_title ON content(title_id);

CREATE TABLE media_file (
    id                  text PRIMARY KEY NOT NULL,
    path                text NOT NULL,
    size                bigint NOT NULL DEFAULT 0,
    languages           text NOT NULL DEFAULT '[]',
    quality             text NOT NULL,
    quality_rank        bigint NOT NULL,
    media_info          text,
    custom_format_score bigint,
    release_type        text
);
CREATE UNIQUE INDEX idx_media_file_path ON media_file(path);

CREATE TABLE content_file (
    content_id    text NOT NULL REFERENCES content(id) ON DELETE CASCADE,
    media_file_id text NOT NULL REFERENCES media_file(id) ON DELETE CASCADE,
    PRIMARY KEY (content_id, media_file_id)
);
CREATE INDEX idx_content_file_file ON content_file(media_file_id);

CREATE TABLE grab (
    id            text PRIMARY KEY NOT NULL,
    content_ref   text NOT NULL,
    release       text NOT NULL,
    indexer_id    text NOT NULL,
    client_id     text NOT NULL,
    category      text NOT NULL,
    download_id   text,
    status        text NOT NULL DEFAULT 'pending',
    created_at    text NOT NULL,
    release_type  text
);
CREATE INDEX idx_grab_status ON grab(status);
CREATE INDEX idx_grab_download ON grab(download_id);

CREATE TABLE history (
    id          text PRIMARY KEY NOT NULL,
    at          text NOT NULL,
    content_id  text NOT NULL,
    run_id      text NOT NULL,
    event       text NOT NULL
);
CREATE INDEX idx_history_content ON history(content_id, at);
CREATE INDEX idx_history_run ON history(run_id);

CREATE TABLE decision_log (
    id          text PRIMARY KEY NOT NULL,
    at          text NOT NULL,
    run_id      text NOT NULL,
    transition  text NOT NULL,
    decision    text,
    note        text
);
CREATE INDEX idx_decision_log_run ON decision_log(run_id, at);

-- ---------------------------------------------------------------------------
-- Configuration / settings
-- ---------------------------------------------------------------------------

CREATE TABLE quality_profile (
    id   text PRIMARY KEY NOT NULL,
    name text NOT NULL,
    body text NOT NULL
);

CREATE TABLE custom_format (
    id    text PRIMARY KEY NOT NULL,
    name  text NOT NULL,
    score bigint NOT NULL DEFAULT 0,
    body  text NOT NULL
);

CREATE TABLE root_folder (
    id      text PRIMARY KEY NOT NULL,
    path    text NOT NULL,
    name    text,
    enabled bigint NOT NULL DEFAULT 1,
    body    text NOT NULL
);
CREATE UNIQUE INDEX idx_root_folder_path ON root_folder(path);

CREATE TABLE indexer (
    id       text PRIMARY KEY NOT NULL,
    name     text NOT NULL,
    kind     text NOT NULL,
    protocol text NOT NULL,
    enabled  bigint NOT NULL DEFAULT 1,
    priority bigint NOT NULL DEFAULT 0,
    body     text NOT NULL
);
CREATE INDEX idx_indexer_enabled ON indexer(enabled);

CREATE TABLE download_client (
    id       text PRIMARY KEY NOT NULL,
    name     text NOT NULL,
    kind     text NOT NULL,
    protocol text NOT NULL,
    enabled  bigint NOT NULL DEFAULT 1,
    priority bigint NOT NULL DEFAULT 0,
    category text NOT NULL,
    body     text NOT NULL
);
CREATE INDEX idx_download_client_enabled ON download_client(enabled);

CREATE TABLE notification (
    id       text PRIMARY KEY NOT NULL,
    name     text NOT NULL,
    kind     text NOT NULL,
    enabled  bigint NOT NULL DEFAULT 1,
    body     text NOT NULL
);
CREATE INDEX idx_notification_enabled ON notification(enabled);
CREATE INDEX idx_notification_kind ON notification(kind);

CREATE TABLE remote_path_mapping (
    id          text PRIMARY KEY NOT NULL,
    host        text NOT NULL DEFAULT '',
    remote_path text NOT NULL,
    local_path  text NOT NULL,
    body        text NOT NULL
);
CREATE INDEX idx_remote_path_mapping_host ON remote_path_mapping(host);

CREATE TABLE delay_profile (
    id      text PRIMARY KEY NOT NULL,
    enabled bigint NOT NULL DEFAULT 1,
    "order" bigint NOT NULL DEFAULT 0,
    body    text NOT NULL
);
CREATE INDEX idx_delay_profile_order ON delay_profile("order");

CREATE TABLE release_profile (
    id      text PRIMARY KEY NOT NULL,
    enabled bigint NOT NULL DEFAULT 1,
    name    text NOT NULL DEFAULT '',
    body    text NOT NULL
);
CREATE INDEX idx_release_profile_name ON release_profile(name);

CREATE TABLE quality_definition (
    name                   text PRIMARY KEY NOT NULL,
    title                  text,
    min_size_per_min       bigint,
    max_size_per_min       bigint,
    preferred_size_per_min bigint,
    body                   text NOT NULL
);

-- ---------------------------------------------------------------------------
-- Cache
-- ---------------------------------------------------------------------------

CREATE TABLE cache (
    cache_key  text PRIMARY KEY NOT NULL,
    value      text NOT NULL,
    expires_at text
);
CREATE INDEX idx_cache_expires ON cache(expires_at);

-- ---------------------------------------------------------------------------
-- Typed identity side-tables
-- ---------------------------------------------------------------------------

CREATE TABLE movie_meta (
    title_id   text PRIMARY KEY NOT NULL,
    title      text NOT NULL,
    sort_title text,
    year       bigint,
    tmdb_id    bigint,
    imdb_id    text,
    overview   text,
    runtime    bigint
);
CREATE INDEX idx_movie_meta_tmdb ON movie_meta(tmdb_id);
CREATE INDEX idx_movie_meta_imdb ON movie_meta(imdb_id);

CREATE TABLE series_meta (
    title_id   text PRIMARY KEY NOT NULL,
    title      text NOT NULL,
    sort_title text,
    year       bigint,
    tvdb_id    bigint,
    tmdb_id    bigint,
    imdb_id    text,
    overview   text
);
CREATE INDEX idx_series_meta_tvdb ON series_meta(tvdb_id);

CREATE TABLE season_meta (
    title_id        text PRIMARY KEY NOT NULL,
    series_title_id text REFERENCES series_meta(title_id) ON DELETE CASCADE,
    season_number   bigint NOT NULL,
    overview        text
);
CREATE INDEX idx_season_meta_series ON season_meta(series_title_id);

CREATE TABLE episode_meta (
    title_id        text PRIMARY KEY NOT NULL,
    series_title_id text REFERENCES series_meta(title_id) ON DELETE CASCADE,
    season_number   bigint NOT NULL,
    episode_number  bigint NOT NULL,
    absolute_number bigint,
    title           text,
    overview        text,
    air_date        text
);
CREATE INDEX idx_episode_meta_series ON episode_meta(series_title_id, season_number, episode_number);

CREATE TABLE content_meta (
    content_id   text PRIMARY KEY NOT NULL REFERENCES content(id) ON DELETE CASCADE,
    title        text,
    year         bigint,
    overview     text,
    runtime      bigint,
    air_date     text,
    digital_date text,
    genres       text,
    rating       double precision,
    rating_votes bigint
);
CREATE INDEX idx_content_meta_air_date ON content_meta(air_date);
CREATE INDEX idx_content_meta_digital_date ON content_meta(digital_date);

-- ---------------------------------------------------------------------------
-- Blocklist / import lists / pending releases
-- ---------------------------------------------------------------------------

CREATE TABLE blocklist (
    id             text PRIMARY KEY NOT NULL,
    content_id     text NOT NULL REFERENCES content(id) ON DELETE CASCADE,
    release_key    text NOT NULL,
    title          text NOT NULL,
    reason         text NOT NULL,
    blocklisted_at text NOT NULL,
    body           text NOT NULL
);
CREATE UNIQUE INDEX idx_blocklist_content_key ON blocklist(content_id, release_key);
CREATE INDEX idx_blocklist_at ON blocklist(blocklisted_at);

CREATE TABLE import_list (
    id          text PRIMARY KEY NOT NULL,
    name        text NOT NULL,
    kind        text NOT NULL,
    enabled     bigint NOT NULL DEFAULT 1,
    media_type  text NOT NULL,
    last_synced text,
    body        text NOT NULL
);
CREATE INDEX idx_import_list_enabled ON import_list(enabled);

CREATE TABLE import_list_exclusion (
    id       text PRIMARY KEY NOT NULL,
    id_type  text NOT NULL,
    id_value text NOT NULL,
    title    text NOT NULL,
    body     text NOT NULL
);
CREATE UNIQUE INDEX idx_import_list_exclusion_key
    ON import_list_exclusion(id_type, id_value);

CREATE TABLE pending_release (
    content_id    text NOT NULL,
    release_key   text NOT NULL,
    first_seen_at bigint NOT NULL,
    protocol      text NOT NULL,
    title         text NOT NULL,
    PRIMARY KEY (content_id, release_key)
);

-- ---------------------------------------------------------------------------
-- Singleton settings documents + auth
-- ---------------------------------------------------------------------------

CREATE TABLE media_management (
    id   bigint PRIMARY KEY NOT NULL CHECK (id = 1),
    body text NOT NULL
);

CREATE TABLE auth_config (
    id   bigint PRIMARY KEY NOT NULL CHECK (id = 1),
    body text NOT NULL
);

CREATE TABLE auth_session (
    token      text PRIMARY KEY NOT NULL,
    username   text NOT NULL,
    created_at bigint NOT NULL,
    expires_at bigint NOT NULL
);
CREATE INDEX idx_auth_session_expires ON auth_session(expires_at);

-- ---------------------------------------------------------------------------
-- Tags
-- ---------------------------------------------------------------------------

CREATE TABLE tag (
    id    bigint PRIMARY KEY NOT NULL,
    label text NOT NULL
);
-- Case-insensitive uniqueness. SQLite spells this `label COLLATE NOCASE`; the
-- portable Postgres equivalent is a functional unique index over LOWER(label).
CREATE UNIQUE INDEX idx_tag_label_nocase ON tag (LOWER(label));

CREATE TABLE content_tag (
    content_id text NOT NULL,
    tag_id     bigint NOT NULL,
    PRIMARY KEY (content_id, tag_id),
    FOREIGN KEY (content_id) REFERENCES content (id) ON DELETE CASCADE,
    FOREIGN KEY (tag_id) REFERENCES tag (id) ON DELETE CASCADE
);
CREATE INDEX idx_content_tag_tag ON content_tag (tag_id);

-- ---------------------------------------------------------------------------
-- Managed-config reconciliation ledger
-- ---------------------------------------------------------------------------

CREATE TABLE managed_config_entity (
    kind         text NOT NULL,
    name         text NOT NULL,
    entity_id    text NOT NULL,
    content_hash text NOT NULL,
    PRIMARY KEY (kind, name)
);

-- ---------------------------------------------------------------------------
-- Full-text search over library titles.
--
-- The SQLite engine uses an FTS5 virtual table (migrations/sqlite/0002); the
-- Postgres equivalent is a real table carrying a generated `tsvector` column
-- indexed with GIN. The repository writes `(content_id, title)` and reads them
-- back exactly as on SQLite; `title_tsv` is maintained automatically. The
-- `simple` text-search config (no stemming/stop-words) mirrors FTS5's plain
-- unicode tokenizer rather than imposing English stemming on titles.
-- ---------------------------------------------------------------------------

CREATE TABLE content_fts (
    content_id text PRIMARY KEY NOT NULL,
    title      text NOT NULL DEFAULT '',
    title_tsv  tsvector GENERATED ALWAYS AS (to_tsvector('simple', title)) STORED
);
CREATE INDEX idx_content_fts_tsv ON content_fts USING GIN (title_tsv);
