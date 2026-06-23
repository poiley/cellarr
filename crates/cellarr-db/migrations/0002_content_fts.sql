-- Full-text search over library titles (docs/08-database.md: no Elasticsearch;
-- UI/library search uses SQLite FTS5).
--
-- This migration is SQLite-specific (FTS5 virtual table). The Postgres engine is
-- deferred behind the `postgres` cargo feature and not yet wired/tested; when it
-- lands, the equivalent will be a tsvector column + GIN index in a
-- dialect-selected migration set. Keeping FTS isolated here makes that swap easy.

CREATE VIRTUAL TABLE content_fts USING fts5(
    content_id UNINDEXED,
    title,
    tokenize = 'unicode61 remove_diacritics 2'
);
