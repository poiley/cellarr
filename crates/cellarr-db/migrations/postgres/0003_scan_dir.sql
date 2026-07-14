-- Remembered directory modification times for the mtime-based incremental rescan
-- walk (see the SQLite 0020 twin for the full rationale). Added as its own
-- migration rather than editing 0001_schema.sql so the existing (checksum-tracked)
-- schema migration is untouched and the live DB applies only this additive change.
CREATE TABLE IF NOT EXISTS scan_dir (
    path  text PRIMARY KEY NOT NULL,
    mtime bigint NOT NULL
);
