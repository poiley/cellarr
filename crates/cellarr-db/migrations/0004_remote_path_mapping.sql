-- Remote-path mappings: a shared layer that rewrites a download client's
-- reported content path into a path cellarr can see, applied once in the jobs
-- runner before Import (see docs/specs/cellarr-download.md). Mirrors the
-- Sonarr/Radarr `RemotePathMapping` the ecosystem (Recyclarr, UoMi) expects, so
-- it gets the same JSON-body-plus-typed-columns treatment as the other config
-- aggregates (docs/02-data-model.md).
CREATE TABLE remote_path_mapping (
    id          TEXT PRIMARY KEY NOT NULL,
    host        TEXT NOT NULL DEFAULT '',
    remote_path TEXT NOT NULL,
    local_path  TEXT NOT NULL,
    -- JSON-serialized RemotePathMapping (authoritative; columns mirror it).
    body        TEXT NOT NULL
);
CREATE INDEX idx_remote_path_mapping_host ON remote_path_mapping(host);
