-- Per-node last-acquisition-search timestamp for the monitored-missing sweep (see
-- the SQLite 0023 twin for the full rationale). Added as its own migration rather
-- than editing 0001_schema.sql so the existing (checksum-tracked) schema migration
-- is untouched and the live DB applies only this additive change.
CREATE TABLE IF NOT EXISTS missing_search (
    content_id  text PRIMARY KEY NOT NULL,
    searched_at text NOT NULL
);
