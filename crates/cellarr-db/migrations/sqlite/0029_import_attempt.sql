-- A completed download whose import keeps failing must not be retried on every
-- reconcile cycle. Three grabs that could never import once turned the job loop
-- into 639 consecutive ReconcileDownloads runs in six hours with zero RssSync:
-- nothing else got scheduled, so nothing was searched for at all.
--
-- Retries are spaced by an exponential backoff off this row rather than capped,
-- deliberately. A cap would abandon a download whose bytes are on disk because of
-- a transient (a database blip lasting hours would burn any fixed number of
-- attempts), and the existing contract is that such a grab is left for a human,
-- never blocklisted. Backing off ends the starvation without ever giving up.
--
-- `last_error` is kept so the reason is visible without trawling logs.
CREATE TABLE IF NOT EXISTS import_attempt (
    grab_id      text PRIMARY KEY NOT NULL,
    attempts     integer NOT NULL,
    last_attempt text NOT NULL,
    last_error   text
);
