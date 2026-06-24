-- Delay profiles: per-protocol grab delays that hold a release for a window so a
-- better one can arrive first (mirrors the Sonarr/Radarr Delay Profile).
--
-- The full `DelayProfile` (preferred protocol, per-protocol delays, bypass flag,
-- tags, order) round-trips losslessly through the JSON `body`. A few typed
-- columns mirror the fields the runner and `/api/v3` shim filter/order on without
-- parsing every row: `enabled` (a disabled profile holds nothing) and `order`
-- (the resolution order — lowest applies first, the catch-all default last).
CREATE TABLE delay_profile (
    id      TEXT PRIMARY KEY NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    -- Resolution order: lower applies first; the tagless catch-all sorts last.
    "order" INTEGER NOT NULL DEFAULT 0,
    -- JSON-serialized DelayProfile (preferred protocol, delays, bypass, tags).
    body    TEXT NOT NULL
);

-- The runner lists profiles in resolution order to pick the governing one; index
-- the order so that ordered read never scans-and-sorts the whole (small) table.
CREATE INDEX IF NOT EXISTS idx_delay_profile_order ON delay_profile("order");

-- Pending releases: the first-seen bookkeeping a delay profile needs.
--
-- When a delay profile holds a grabbable release, the runner records (content,
-- release-key) -> the unix time it was first seen. The next run reads it back to
-- compute how long the release has been waiting, so the delay is measured from
-- when cellarr *first observed* the release, not from process start. The row is
-- cleared once the release is grabbed (or it ages out with the content). The
-- composite primary key makes the upsert "remember the earliest sighting"
-- idempotent — re-seeing the same release never resets its clock.
CREATE TABLE pending_release (
    content_id     TEXT NOT NULL,
    release_key    TEXT NOT NULL,
    -- Unix seconds the release was first seen for this content.
    first_seen_at  INTEGER NOT NULL,
    -- The release protocol (usenet/torrent), for diagnostics.
    protocol       TEXT NOT NULL,
    -- The advertised title, for the held-releases UI.
    title          TEXT NOT NULL,
    PRIMARY KEY (content_id, release_key)
);

