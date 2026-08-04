-- The decision-log retention sweep filters on `at`, which was unindexed, so its
-- nightly `count(*) WHERE at < cutoff` sequential-scanned the whole table: 3.5s on
-- 178k rows, tripping the slow-statement warning on every run, and the DELETE that
-- follows scanned it again. Both become index lookups.
--
-- `at` is RFC3339 text, which sorts lexicographically in the same order as the
-- instants it encodes (fixed-width, zero-padded, UTC), so a plain B-tree on it
-- answers the range predicate correctly.
CREATE INDEX IF NOT EXISTS idx_decision_log_at ON decision_log(at);
