-- Search fatigue: how many consecutive sweeps have searched this node and found
-- NOTHING AT ALL, and when it is next worth trying.
--
-- 773 of 1088 missing items on the reference library had been searched repeatedly
-- for three weeks without a single candidate release ever being returned — 91% of
-- them episodes of two reality shows the indexers simply do not carry. They
-- rotated through the bounded per-sweep budget at the same priority as obtainable
-- content, so most of the search budget (and the indexer request budget, which is
-- rate-limited) was spent on content that could never be found.
--
-- `fruitless` counts consecutive searches that returned zero releases; ANY
-- candidate resets it, so a show that starts being carried recovers immediately.
-- `next_due_at` is the backoff computed from it at write time — kept as a stamp
-- rather than computed in the query so the ordering stays portable between SQLite
-- and Postgres.
ALTER TABLE missing_search ADD COLUMN fruitless bigint NOT NULL DEFAULT 0;
ALTER TABLE missing_search ADD COLUMN next_due_at text;
