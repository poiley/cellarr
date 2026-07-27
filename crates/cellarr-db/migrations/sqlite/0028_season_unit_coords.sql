-- Give season nodes a season coordinate.
--
-- A season container was stamped with an episode coordinate carrying a sentinel
-- `episode: 0`. That made a season indistinguishable from "episode zero of a
-- season" to anything reading coordinates: it cannot be searched (an indexer
-- query would ask for E00), and a season-pack release could never be matched to
-- it. `seasonpack` is the coordinate that already describes a whole season — the
-- shape the parser produces for a pack — so a season unit and the release that
-- fills it now describe the same thing.
--
-- Only season-kind rows are touched, and only those still holding the old
-- episode-shaped coordinate, so the migration is idempotent.
UPDATE content
SET coords = json_object('type', 'seasonpack', 'season', json_extract(coords, '$.season'))
WHERE kind = 'season'
  AND json_extract(coords, '$.type') = 'episode'
  AND json_extract(coords, '$.season') IS NOT NULL;
