-- High-water download progress per in-flight grab, for fast stalled-download
-- detection in the reconcile sweep (see the SQLite 0022 twin for the full
-- rationale). Added as its own migration rather than editing 0001_schema.sql so
-- the existing (checksum-tracked) schema migration is untouched and the live DB
-- applies only this additive change.
CREATE TABLE IF NOT EXISTS download_progress (
    grab_id    text PRIMARY KEY NOT NULL,
    progress   double precision NOT NULL,
    updated_at text NOT NULL
);
