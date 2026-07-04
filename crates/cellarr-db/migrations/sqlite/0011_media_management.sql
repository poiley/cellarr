-- Media-management settings: the singleton, library-wide file-handling policy.
--
-- This holds the cross-library `MediaManagement` aggregate (recycle bin, the
-- per-media-type naming formats, the chmod/chown permission policy, and the
-- extra-file import policy). There is exactly one logical settings document, so the
-- table is a single-row store keyed on a constant `id` (a CHECK pins it to 1), and
-- the whole `MediaManagement` round-trips losslessly through the JSON `body` — the
-- same "typed where shared, JSON for the open-ended remainder" model the other
-- config aggregates use (docs/02-data-model.md). An absent row means "defaults",
-- so a zero-config library behaves exactly as before this table existed.
CREATE TABLE media_management (
    -- Pinned to 1: a single settings document, upserted in place.
    id   INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    -- JSON-serialized MediaManagement (recycle bin, naming, permissions, extras).
    body TEXT NOT NULL
);
