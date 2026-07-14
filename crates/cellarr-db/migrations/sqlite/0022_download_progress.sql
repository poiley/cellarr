-- Tracks the high-water download progress of each in-flight grab so the reconcile
-- sweep can spot a download that has STALLED — made no progress for a while — and
-- clean it (blocklist + re-acquire) long before the coarse 24h/zero-peer rule
-- would. A torrent limping along with a peer or two that never actually completes
-- otherwise holds a download-concurrency slot for a full day, throttling all new
-- acquisition (and the upgrade sweep) behind it.
--
-- `progress` is the last observed fraction complete (0.0–1.0); `updated_at` is
-- when it last ADVANCED. Each reconcile compares the client's current progress to
-- this high-water mark: advanced → bump both; unchanged for the stall window →
-- dead. Persisted (not in-memory) so the timer survives a daemon restart — an
-- in-memory timer would reset every restart and never reach the threshold. The row
-- is cleared when the grab reaches a terminal state.
CREATE TABLE IF NOT EXISTS download_progress (
    grab_id    TEXT PRIMARY KEY NOT NULL,
    progress   REAL NOT NULL,
    updated_at TEXT NOT NULL
);
