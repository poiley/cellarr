-- Per-file last auto-onboard attempt timestamp (see the SQLite 0024 twin for the
-- full rationale). Additive: a new nullable column so the checksum-tracked 0001
-- schema is untouched and the live DB applies only this change.
ALTER TABLE unmatched_scan ADD COLUMN last_attempt text;
