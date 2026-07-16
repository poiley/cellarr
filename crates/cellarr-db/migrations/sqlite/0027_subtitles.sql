-- Subtitle sidecar files fetched for a media file — one row per
-- (file, language, forced/hearing-impaired variant). Attaches to media_file
-- (ON DELETE CASCADE) so removing a file clears its subtitle records; the
-- on-disk .srt itself is cleaned by the fs layer, not this cascade.
--   forced / hearing_impaired: 0/1 flags distinguishing the accessibility variants
--   score:       the provider's match score, when it supplies one (nullable)
--   provider_id: the provider's own id for the chosen file (for re-download/upgrade)
--   added_at:    RFC3339 UTC when the sidecar was written
CREATE TABLE IF NOT EXISTS subtitle (
    id               TEXT PRIMARY KEY NOT NULL,
    media_file_id    TEXT NOT NULL REFERENCES media_file(id) ON DELETE CASCADE,
    language         TEXT NOT NULL,
    path             TEXT NOT NULL,
    provider         TEXT NOT NULL,
    provider_id      TEXT,
    score            INTEGER,
    forced           INTEGER NOT NULL DEFAULT 0,
    hearing_impaired INTEGER NOT NULL DEFAULT 0,
    added_at         TEXT NOT NULL
);

-- One subtitle per (file, language, variant): a re-fetch UPSERTs the existing row.
CREATE UNIQUE INDEX IF NOT EXISTS idx_subtitle_file_variant
    ON subtitle(media_file_id, language, forced, hearing_impaired);
CREATE INDEX IF NOT EXISTS idx_subtitle_media_file ON subtitle(media_file_id);
