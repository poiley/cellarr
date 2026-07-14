-- Remembers when each content node was last considered for a QUALITY UPGRADE, so
-- the automatic upgrade sweep rotates fairly through the backlog instead of
-- re-searching the same nodes every run (which would hammer the indexers).
--
-- The upgrade sweep takes the least-recently-searched candidates first (a node
-- with no row here has never been searched and sorts ahead of any that have), a
-- bounded batch per run. After running a node through the decision it upserts the
-- node's `searched_at` here, so the next run moves on to the next slice. This is
-- the upgrade counterpart to the acquisition sweep's monitored-missing set: it
-- targets monitored leaf nodes that already HAVE a file but may have a better
-- release available.
CREATE TABLE IF NOT EXISTS upgrade_search (
    content_id  TEXT PRIMARY KEY NOT NULL,
    searched_at TEXT NOT NULL
);
