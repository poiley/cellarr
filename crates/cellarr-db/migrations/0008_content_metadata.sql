-- Content-scoped persisted metadata (the identify/refresh write seam).
--
-- The typed identity side-tables (movie_meta/series_meta/episode_meta) are keyed
-- by `title_id` and model the canonical *identity* of a title. Identify/Refresh,
-- however, run against a concrete `content` node and need a place to durably
-- attach the resolved facts (year, overview, runtime, air/release dates, artwork
-- urls) to *that node* without first minting and linking a title_id — which the
-- pipeline does not yet do. This table is that content-scoped seam: one row per
-- content node, written when the node is identified or refreshed, read by the
-- content-detail endpoint, the v3 movie/series detail, and the calendar.
--
-- Dates are stored in ISO `yyyy-mm-dd` text form (string-comparable, which is
-- what the calendar windowing relies on). All fields except the id are nullable
-- so a partial identify (title only) still persists a row.
CREATE TABLE content_meta (
    content_id      TEXT PRIMARY KEY NOT NULL REFERENCES content(id) ON DELETE CASCADE,
    title           TEXT,
    year            INTEGER,
    overview        TEXT,
    -- Runtime in minutes (a movie's running time, or an episode's length).
    runtime         INTEGER,
    -- A movie's theatrical/physical release date; an episode's air date.
    air_date        TEXT,
    -- A movie's digital (streaming/home) release date, when distinct from the
    -- theatrical release. Null for episodes.
    digital_date    TEXT
);

-- The calendar windows on the dates, so index them for the range scan.
CREATE INDEX idx_content_meta_air_date ON content_meta(air_date);
CREATE INDEX idx_content_meta_digital_date ON content_meta(digital_date);
