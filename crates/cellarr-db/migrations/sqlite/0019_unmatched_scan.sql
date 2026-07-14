-- Remembered UNMATCHED library files, so the background rescan does not
-- re-process the same never-placeable files (extras, samples, foreign-named
-- media the parser cannot map to any node) on every run.
--
-- A library rescan walks every root and, for each untracked video file, parses +
-- tries to place it — and, with auto-onboard on, looks its title up in the
-- metadata source. A large library accumulates thousands of files that will never
-- match a node; re-doing that walk + lookup every run took ~15 min and starved the
-- single-threaded job loop. Files listed here are skipped on subsequent scans
-- (same as already-tracked files). A NEW file always has a new path, so it is
-- never in this table and is still scanned; the manual-import screen ignores this
-- table so a user can still see and place these files by hand.
CREATE TABLE IF NOT EXISTS unmatched_scan (
    path       TEXT PRIMARY KEY NOT NULL,
    size       INTEGER NOT NULL,
    first_seen TEXT NOT NULL
);
