-- When each series was last re-resolved by the metadata refresh (RFC3339 UTC),
-- NULL until first refreshed. MetadataRefresh re-fetches + re-expands EVERY series
-- each run (to pick up newly-aired episodes) — on a large library that read/write
-- burst can saturate a small database. Ordering by this stamp (never-refreshed
-- first, then least-recently) and taking a bounded batch per run spreads the work
-- over successive runs so every series is still refreshed within a bounded window.
CREATE TABLE IF NOT EXISTS series_refresh (
    content_id  TEXT PRIMARY KEY NOT NULL,
    resolved_at TEXT NOT NULL
);
