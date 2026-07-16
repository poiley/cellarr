-- Per-series last-refresh timestamp for the bounded round-robin metadata refresh
-- (see the SQLite 0026 twin). Additive; the checksum-tracked 0001 schema is untouched.
CREATE TABLE IF NOT EXISTS series_refresh (
    content_id  text PRIMARY KEY NOT NULL,
    resolved_at text NOT NULL
);
