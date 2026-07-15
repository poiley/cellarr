-- Normalize the FTS index so titles with separators are searchable. The original
-- generated column indexed `to_tsvector('simple', title)`, which keeps "11.22.63"
-- as a SINGLE lexeme — unreachable from the parser's dots-as-spaces "11 22 63"
-- query. Fold every run of non-alphanumerics to a space before tokenizing, so the
-- index holds `11 22 63` / `obi wan kenobi`; the query side normalizes identically
-- (see ContentRepo::search). `title` itself is untouched (still the display title
-- read by `title_for`); only the derived tsvector changes.
--
-- Additive: a new migration that redefines the DERIVED column (the underlying
-- `title` data is preserved and the column recomputes for every row); the
-- checksum-tracked 0001 schema is untouched. SQLite needs no twin — its FTS5
-- tokenizer already splits on non-alphanumerics.
DROP INDEX IF EXISTS idx_content_fts_tsv;
ALTER TABLE content_fts DROP COLUMN title_tsv;
ALTER TABLE content_fts ADD COLUMN title_tsv tsvector
    GENERATED ALWAYS AS (to_tsvector('simple', regexp_replace(title, '[^[:alnum:]]+', ' ', 'g'))) STORED;
CREATE INDEX idx_content_fts_tsv ON content_fts USING GIN (title_tsv);
