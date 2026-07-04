-- Web-UI authentication: the single-admin auth config and the Forms session store.
--
-- cellarr mirrors Sonarr/Radarr's "Authentication" setting minus multi-user: there
-- is exactly ONE admin account (a username + a password *hash*) and a chosen
-- method (none | forms | basic). So the config is a single-row document keyed on a
-- constant id (a CHECK pins it to 1), the same single-row pattern media_management
-- uses (docs/02-data-model.md). The whole core::AuthConfig round-trips losslessly
-- through the JSON `body`; an absent row means "open" (method=none, no credential),
-- so a zero-config install is usable immediately.
--
-- SECURITY: the `body` carries the password HASH (an Argon2 PHC string), never the
-- plaintext password — the plaintext is not even modelled in core, so it can never
-- reach this column.
CREATE TABLE auth_config (
    -- Pinned to 1: a single auth-settings document, upserted in place.
    id   INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    -- JSON-serialized core::AuthConfig (method, username, password hash).
    body TEXT NOT NULL
);

-- Forms-auth sessions: an unguessable CSPRNG token minted on a successful login,
-- exchanged via an HttpOnly cookie. A session is valid until its `expires_at`
-- (unix seconds); logout deletes the row. Keyed on the opaque token so a cookie
-- presentation is a single indexed point-lookup, and so a stolen DB row reveals
-- only an already-expired-or-revocable token (no password material lives here).
CREATE TABLE auth_session (
    -- The opaque session token (CSPRNG, URL-safe). The cookie value.
    token       TEXT PRIMARY KEY NOT NULL,
    -- The admin username this session authenticates (single-user, but stored so a
    -- credential change can invalidate stale sessions by username if needed).
    username    TEXT NOT NULL,
    -- When the session was created (unix seconds), for auditing/cleanup.
    created_at  INTEGER NOT NULL,
    -- When the session expires (unix seconds); a request after this is rejected
    -- and the row is a candidate for sweep-deletion.
    expires_at  INTEGER NOT NULL
);
CREATE INDEX idx_auth_session_expires ON auth_session(expires_at);
