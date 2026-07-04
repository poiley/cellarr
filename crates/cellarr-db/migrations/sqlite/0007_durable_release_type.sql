-- Durable release type (the season-pack re-grab-loop fix).
--
-- The parsed release type / full-season flag is derived ONCE at grab time and
-- written down here, so the upgrade/reconcile decision reads it back instead of
-- re-parsing the advertised title every cycle (re-parsing is what made the
-- originals loop forever grabbing-then-rejecting the same season pack).
--
-- Stored as the snake_case scalar `cellarr_core::ReleaseType` serializes to
-- (e.g. 'full_season'), nullable so rows written before this migration keep
-- working (a NULL release type falls back to the prior quality/CF-only logic).

ALTER TABLE grab ADD COLUMN release_type TEXT;
ALTER TABLE media_file ADD COLUMN release_type TEXT;
