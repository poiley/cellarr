-- When each remembered-unmatched file was last tried by AUTO-ONBOARD (RFC3339
-- UTC), NULL until tried. The adopt pass records files it can't place into
-- unmatched_scan; auto-onboard drains that backlog, but must not re-run a metadata
-- lookup for a file it already failed to onboard every rescan (a stable library's
-- un-onboardable extras/samples would hammer the metadata provider forever). Pending
-- (last_attempt IS NULL) files are onboarded; a failure stamps last_attempt so the
-- file is skipped next pass; a success deletes the row. So the backlog drains once
-- and the sweep goes quiet — no manual cache-clearing needed.
ALTER TABLE unmatched_scan ADD COLUMN last_attempt TEXT;
