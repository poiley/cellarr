-- Adopt cellarr-core v2: persist the expanded config aggregates and the grab
-- lifecycle.
--
-- The config tables in 0001 were stubs (only the columns the very first pass
-- needed). cellarr-core now ships typed config structs — RootFolder,
-- IndexerConfig, DownloadClientConfig, NotificationConfig — each with a small set
-- of common fields plus an open-ended `settings: serde_json::Value`. Following
-- docs/02-data-model.md, we store the whole serialized struct as JSON in a `body`
-- column and keep only the columns we actually index/filter on as typed columns.
-- Rebuilding the tables (rather than ALTERing) keeps the shape clean and the
-- stub tables held no real rows yet.
--
-- The grab table's status default also moves from the old placeholder 'queued'
-- to core's initial GrabStatus 'pending'.

-- ---------------------------------------------------------------------------
-- Root folders
-- ---------------------------------------------------------------------------
DROP TABLE IF EXISTS root_folder;
CREATE TABLE root_folder (
    id         TEXT PRIMARY KEY NOT NULL,
    path       TEXT NOT NULL,
    name       TEXT,
    enabled    INTEGER NOT NULL DEFAULT 1,
    -- JSON-serialized RootFolder (the authoritative copy; typed columns above
    -- mirror it for indexing/listing).
    body       TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_root_folder_path ON root_folder(path);

-- ---------------------------------------------------------------------------
-- Indexers
-- ---------------------------------------------------------------------------
DROP TABLE IF EXISTS indexer;
CREATE TABLE indexer (
    id       TEXT PRIMARY KEY NOT NULL,
    name     TEXT NOT NULL,
    kind     TEXT NOT NULL,
    protocol TEXT NOT NULL,
    enabled  INTEGER NOT NULL DEFAULT 1,
    priority INTEGER NOT NULL DEFAULT 0,
    -- JSON-serialized IndexerConfig; secrets within (API keys) are stored
    -- encrypted at rest and never logged.
    body     TEXT NOT NULL
);
CREATE INDEX idx_indexer_enabled ON indexer(enabled);

-- ---------------------------------------------------------------------------
-- Download clients
-- ---------------------------------------------------------------------------
DROP TABLE IF EXISTS download_client;
CREATE TABLE download_client (
    id       TEXT PRIMARY KEY NOT NULL,
    name     TEXT NOT NULL,
    kind     TEXT NOT NULL,
    protocol TEXT NOT NULL,
    enabled  INTEGER NOT NULL DEFAULT 1,
    priority INTEGER NOT NULL DEFAULT 0,
    category TEXT NOT NULL,
    -- JSON-serialized DownloadClientConfig; credentials encrypted at rest.
    body     TEXT NOT NULL
);
CREATE INDEX idx_download_client_enabled ON download_client(enabled);

-- ---------------------------------------------------------------------------
-- Notifications
-- ---------------------------------------------------------------------------
DROP TABLE IF EXISTS notification;
CREATE TABLE notification (
    id       TEXT PRIMARY KEY NOT NULL,
    name     TEXT NOT NULL,
    kind     TEXT NOT NULL,
    enabled  INTEGER NOT NULL DEFAULT 1,
    -- JSON-serialized NotificationConfig; secrets encrypted at rest.
    body     TEXT NOT NULL
);
CREATE INDEX idx_notification_enabled ON notification(enabled);

-- ---------------------------------------------------------------------------
-- Media files: core's MediaFile carries an optional custom_format_score (None
-- until the decision engine scores the file) and a Quality{name,rank}. The 0001
-- table made custom_format_score NOT NULL DEFAULT 0, which cannot represent the
-- "not yet scored" state, and stored quality as loose JSON. Rebuild it so the
-- optionality round-trips: `quality` holds the JSON Quality, `quality_rank`
-- mirrors its rank for fast ordering, and `custom_format_score` is nullable.
-- (No rows exist yet, so the rebuild loses nothing.)
-- ---------------------------------------------------------------------------
DROP INDEX IF EXISTS idx_media_file_path;
DROP TABLE IF EXISTS content_file;
DROP TABLE IF EXISTS media_file;
CREATE TABLE media_file (
    id          TEXT PRIMARY KEY NOT NULL,
    path        TEXT NOT NULL,
    size        INTEGER NOT NULL DEFAULT 0,
    -- JSON array of language codes.
    languages   TEXT NOT NULL DEFAULT '[]',
    -- JSON: the assessed Quality{name,rank} for this file.
    quality     TEXT NOT NULL,
    -- Mirrors quality.rank for fast ordering without parsing the JSON.
    quality_rank INTEGER NOT NULL,
    -- JSON: extracted media-info (codecs, runtime, …); open-ended, so JSON.
    media_info  TEXT,
    -- NULL until the decision engine has scored the file.
    custom_format_score INTEGER
);
CREATE UNIQUE INDEX idx_media_file_path ON media_file(path);

-- Re-create the many-to-many link dropped above so it references the rebuilt
-- media_file table.
CREATE TABLE content_file (
    content_id    TEXT NOT NULL REFERENCES content(id) ON DELETE CASCADE,
    media_file_id TEXT NOT NULL REFERENCES media_file(id) ON DELETE CASCADE,
    PRIMARY KEY (content_id, media_file_id)
);
CREATE INDEX idx_content_file_file ON content_file(media_file_id);

-- ---------------------------------------------------------------------------
-- Grab lifecycle: core's initial status is 'pending', not the old 'queued'.
-- ---------------------------------------------------------------------------
DROP INDEX IF EXISTS idx_grab_status;
DROP INDEX IF EXISTS idx_grab_download;
ALTER TABLE grab RENAME TO grab_old;
CREATE TABLE grab (
    id            TEXT PRIMARY KEY NOT NULL,
    content_ref   TEXT NOT NULL,
    release       TEXT NOT NULL,
    indexer_id    TEXT NOT NULL,
    client_id     TEXT NOT NULL,
    category      TEXT NOT NULL,
    download_id   TEXT,
    status        TEXT NOT NULL DEFAULT 'pending',
    created_at    TEXT NOT NULL
);
INSERT INTO grab (id, content_ref, release, indexer_id, client_id, category,
                  download_id, status, created_at)
    SELECT id, content_ref, release, indexer_id, client_id, category,
           download_id, status, created_at
    FROM grab_old;
DROP TABLE grab_old;
CREATE INDEX idx_grab_status ON grab(status);
CREATE INDEX idx_grab_download ON grab(download_id);
