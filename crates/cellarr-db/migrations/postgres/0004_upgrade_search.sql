-- Per-node last-upgrade-search timestamp for the automatic upgrade sweep (see the
-- SQLite 0021 twin for the full rationale). Added as its own migration rather than
-- editing 0001_schema.sql so the existing (checksum-tracked) schema migration is
-- untouched and the live DB applies only this additive change.
CREATE TABLE IF NOT EXISTS upgrade_search (
    content_id  text PRIMARY KEY NOT NULL,
    searched_at text NOT NULL
);
