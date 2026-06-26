-- Release profiles: required / ignored / preferred terms matched against a
-- release title, tag-scoped to content (mirrors the Sonarr Release Profile).
--
-- The full `ReleaseProfile` (name, enabled, tag ids, required/ignored/preferred
-- terms) round-trips losslessly through the JSON `body`. A couple of typed
-- columns mirror the fields the decision path and `/api/v3` shim filter/order on
-- without parsing every row: `enabled` (a disabled profile gates and scores
-- nothing) and `name` (for an ordered listing).
CREATE TABLE release_profile (
    id      TEXT PRIMARY KEY NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    name    TEXT NOT NULL DEFAULT '',
    -- JSON-serialized ReleaseProfile (tags, required, ignored, preferred terms).
    body    TEXT NOT NULL
);

-- The shim lists profiles by name; index it so the ordered read never
-- scans-and-sorts the whole (small) table.
CREATE INDEX IF NOT EXISTS idx_release_profile_name ON release_profile(name);
