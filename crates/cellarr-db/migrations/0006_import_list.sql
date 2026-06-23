-- Import lists + list exclusions.
--
-- An import list pulls a curated set of items (Trakt list, TMDb collection, Plex
-- watchlist, …) and the sync job adds the monitored ones cellarr does not already
-- have. The empty-vs-failed-fetch safeguard lives in cellarr-core
-- (sync_import_list): a failed fetch never drives a clean action, and
-- last_successful_sync is stamped only on a confirmed-good fetch. That timestamp
-- is mirrored into a typed column here so a future clean path can require a recent
-- good sync cheaply. See docs/06-integrations.md and docs/parity (Phase F).
--
-- Following docs/02-data-model.md, the authoritative copy of each row is the
-- serialized JSON in `body`; the columns we filter/order on are mirrored.

CREATE TABLE import_list (
    id            TEXT PRIMARY KEY NOT NULL,
    name          TEXT NOT NULL,
    kind          TEXT NOT NULL,
    enabled       INTEGER NOT NULL DEFAULT 1,
    media_type    TEXT NOT NULL,
    -- The last confirmed-good sync time (RFC3339), or NULL if never. Stamped only
    -- on success so a failed fetch can never look like a recent good sync.
    last_synced   TEXT,
    -- JSON-serialized ImportListConfig (authoritative; columns mirror it).
    body          TEXT NOT NULL
);

-- The sync job reads enabled lists; CRUD lists by name.
CREATE INDEX idx_import_list_enabled ON import_list(enabled);

-- List exclusions: items the user never wants any import list to re-add, keyed by
-- external id so an excluded entry is skipped on every future sync.
CREATE TABLE import_list_exclusion (
    id        TEXT PRIMARY KEY NOT NULL,
    id_type   TEXT NOT NULL,
    id_value  TEXT NOT NULL,
    title     TEXT NOT NULL,
    -- JSON-serialized ImportListExclusion (the authoritative copy).
    body      TEXT NOT NULL
);

-- The sync consults exclusions by (id_type, id_value); make that pair unique so
-- re-excluding the same item refreshes the row rather than duplicating it.
CREATE UNIQUE INDEX idx_import_list_exclusion_key
    ON import_list_exclusion(id_type, id_value);
