-- Quality definitions: the per-quality, editable size bounds the decision engine
-- enforces (and the title the UI / `/api/v3` shim shows).
--
-- The quality *catalogue* (which qualities exist and their worst→best `rank`)
-- stays a code-owned constant: a deployment never invents new quality buckets, so
-- the ranking ships as `QualityRanking::default()`. What a user CAN edit are the
-- per-quality knobs the originals expose — the display title and the
-- min/max/preferred size-per-minute. Those edits are the only thing persisted
-- here, keyed by the quality's stable canonical `name`; the loader merges them
-- onto the default ranking, leaving `rank` (and thus all ordering) untouched.
--
-- A row exists only for a quality whose knobs were edited away from their
-- defaults; an absent row means "use the code default" (all-`None`). The size
-- columns are bytes-per-minute and nullable (an unset bound does not gate). The
-- full `QualityDefinition` round-trips losslessly through the JSON `body`, the
-- same pattern the other config aggregates use.
CREATE TABLE quality_definition (
    -- The quality's stable canonical name (e.g. "Bluray-1080p"); the identity the
    -- loader merges edits onto and the decision engine resolves a parse to.
    name                   TEXT PRIMARY KEY NOT NULL,
    -- Editable display title (NULL = show the canonical name).
    title                  TEXT,
    -- Minimum size, bytes per minute (NULL = no lower bound).
    min_size_per_min       INTEGER,
    -- Maximum size, bytes per minute (NULL = no upper bound).
    max_size_per_min       INTEGER,
    -- Advisory preferred size, bytes per minute (NULL = unset; never gates).
    preferred_size_per_min INTEGER,
    -- JSON-serialized QualityDefinition (carries the merged-with-default rank too).
    body                   TEXT NOT NULL
);
