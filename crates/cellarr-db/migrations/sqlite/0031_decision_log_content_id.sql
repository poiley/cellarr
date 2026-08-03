-- Make the decision log queryable BY CONTENT.
--
-- The log already records which content each decision was about, but only inside
-- the `decision` JSON blob — and the JSON accessors differ between SQLite and
-- Postgres, so nothing portable could ask "why is this item still missing?". That
-- question is the whole value of an audit log: on the reference library 773 of
-- 1088 missing items had never had a single candidate release, 109 had only dead
-- torrents, and 20 were gettable but blocked by the quality profile — three very
-- different situations that looked identical in the UI.
--
-- A real column makes that query plain SQL, indexable, and portable, and lets
-- retention prune per content rather than scanning a JSON blob.
--
-- Existing rows are backfilled here with the dialect-native accessor; rows whose
-- decision carries no content_ref (stage transitions) stay NULL, which is correct.
ALTER TABLE decision_log ADD COLUMN content_id text;

UPDATE decision_log
   SET content_id = json_extract(decision, '$.content_ref.id')
 WHERE decision IS NOT NULL AND content_id IS NULL;

CREATE INDEX IF NOT EXISTS idx_decision_log_content ON decision_log(content_id);
