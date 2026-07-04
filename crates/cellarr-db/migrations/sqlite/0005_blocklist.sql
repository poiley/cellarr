-- The failed-download blocklist.
--
-- When a download fails (or a grab is manually marked failed), the release is
-- recorded here so the decision/grab path never re-grabs the same bad release.
-- An entry is keyed by a stable release_key (derived from the release's indexer
-- GUID / download URL / title) scoped to the content node it was grabbed for, so
-- a re-search that re-discovers the identical release recognizes and skips it.
-- See docs/03-pipeline.md (download-failed -> blocklist + re-search) and
-- cellarr-core::blocklist.
--
-- Following docs/02-data-model.md, the whole serialized BlocklistEntry is stored
-- as JSON in `body`; the columns we filter/order on are mirrored as typed
-- columns. The (content_id, release_key) pair is unique so re-blocklisting the
-- same release for the same content refreshes the row rather than duplicating it.

CREATE TABLE blocklist (
    id             TEXT PRIMARY KEY NOT NULL,
    content_id     TEXT NOT NULL REFERENCES content(id) ON DELETE CASCADE,
    release_key    TEXT NOT NULL,
    title          TEXT NOT NULL,
    reason         TEXT NOT NULL,
    blocklisted_at TEXT NOT NULL,
    -- JSON-serialized BlocklistEntry (the authoritative copy).
    body           TEXT NOT NULL
);

-- The lookup the decision/grab path makes (is this release blocklisted for this
-- content?) and the idempotency key for `add`.
CREATE UNIQUE INDEX idx_blocklist_content_key ON blocklist(content_id, release_key);

-- The /api/v3/blocklist list orders newest first.
CREATE INDEX idx_blocklist_at ON blocklist(blocklisted_at);
