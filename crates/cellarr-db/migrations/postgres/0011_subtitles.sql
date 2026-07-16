-- Subtitle sidecars per media file (see the SQLite 0027 twin). Additive; the
-- checksum-tracked 0001 schema is untouched. One row per (file, language,
-- forced/hearing-impaired variant); attaches to media_file ON DELETE CASCADE.
CREATE TABLE IF NOT EXISTS subtitle (
    id               text PRIMARY KEY NOT NULL,
    media_file_id    text NOT NULL REFERENCES media_file(id) ON DELETE CASCADE,
    language         text NOT NULL,
    path             text NOT NULL,
    provider         text NOT NULL,
    provider_id      text,
    score            bigint,
    forced           bigint NOT NULL DEFAULT 0,
    hearing_impaired bigint NOT NULL DEFAULT 0,
    added_at         text NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_subtitle_file_variant
    ON subtitle(media_file_id, language, forced, hearing_impaired);
CREATE INDEX IF NOT EXISTS idx_subtitle_media_file ON subtitle(media_file_id);
