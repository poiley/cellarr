-- Rich-metadata columns on content_meta.
--
-- The identify/refresh path resolves more than the display facts already stored
-- here: genres, and the primary source's user rating. Posters/fanart are bytes
-- in the MediaCover artwork cache (not a column), so only the text/numeric facts
-- live here.
--
-- `genres` is a JSON array of strings (e.g. ["Animation","Comedy"]) so the
-- list-valued field needs no association table; the read path decodes it. `rating`
-- is the TMDB vote_average on a 0..10 scale and `rating_votes` its vote_count.
-- All nullable so a partial identify still persists a usable row.
ALTER TABLE content_meta ADD COLUMN genres TEXT;
ALTER TABLE content_meta ADD COLUMN rating REAL;
ALTER TABLE content_meta ADD COLUMN rating_votes INTEGER;
