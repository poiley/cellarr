-- Remembered UNMATCHED library files (see the SQLite 0019 twin for the full
-- rationale). Added as its own migration rather than editing 0001_schema.sql so
-- the existing (checksum-tracked) schema migration is untouched and the live DB
-- applies only this additive change.
CREATE TABLE IF NOT EXISTS unmatched_scan (
    path       text PRIMARY KEY NOT NULL,
    size       bigint NOT NULL,
    first_seen text NOT NULL
);
