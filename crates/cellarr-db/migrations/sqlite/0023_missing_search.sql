-- Remembers when each monitored-MISSING node was last run through an acquisition
-- search, so the bounded per-run sweep (MissingItemSearch / RssSync) rotates fairly
-- through the whole backlog instead of re-drawing a random subset every run (which
-- left grabbable-but-unlucky items un-searched indefinitely).
--
-- The sweep takes the least-recently-searched nodes first (a node with no row here
-- has never been searched and sorts ahead of any that have), a bounded batch per
-- run, then upserts each searched node's `searched_at` here so the next run moves on
-- to the next slice — draining the backlog in bounded time. This is the acquisition
-- counterpart to `upgrade_search` (which does the same for monitored leaves that
-- already HAVE a file but may have a better release available).
CREATE TABLE IF NOT EXISTS missing_search (
    content_id  TEXT PRIMARY KEY NOT NULL,
    searched_at TEXT NOT NULL
);
