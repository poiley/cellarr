-- Give season nodes a season coordinate. See the SQLite migration of the same
-- name for why; this is the same rewrite in Postgres' JSON dialect.
UPDATE content
SET coords = json_build_object(
        'type', 'seasonpack',
        'season', (coords::json ->> 'season')::bigint
    )::text
WHERE kind = 'season'
  AND (coords::json ->> 'type') = 'episode'
  AND (coords::json ->> 'season') IS NOT NULL;
