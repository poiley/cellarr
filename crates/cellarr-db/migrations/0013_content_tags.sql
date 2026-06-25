-- Tags made real: a persisted tag vocabulary and the content↔tag association.
--
-- Sonarr/Radarr expose a small `tag` resource (`{ id, label }`) the ecosystem
-- round-trips, and tag-scope a content item's delay profile, indexers, download
-- clients, and notifications. Until now cellarr's `/tag` CRUD was backed by an
-- in-process, ephemeral store and the content model carried no tags at all, so a
-- tag-scoped delay profile could never match. These two tables make tags
-- persistent and associate them with content, so the pipeline resolves a node's
-- real tags and honours tag-scoped routing.

-- The tag vocabulary: a stable integer id (the *arr convention; ids start at 1,
-- id 0 is never used) and a label, deduplicated case-insensitively. The label is
-- stored as written; uniqueness is enforced case-insensitively via the index
-- below so "Anime" and "anime" never both exist.
CREATE TABLE tag (
    id    INTEGER PRIMARY KEY NOT NULL,
    label TEXT NOT NULL
);

-- Case-insensitive uniqueness on the label, matching the originals' de-dup.
CREATE UNIQUE INDEX idx_tag_label_nocase ON tag (label COLLATE NOCASE);

-- The content↔tag association. A content node can carry any number of tags; a
-- tag can be on any number of nodes. The row falls away when its content node is
-- deleted (ON DELETE CASCADE), and when its tag is deleted, so a removed tag
-- detaches from every node it tagged.
CREATE TABLE content_tag (
    content_id TEXT NOT NULL,
    tag_id     INTEGER NOT NULL,
    PRIMARY KEY (content_id, tag_id),
    FOREIGN KEY (content_id) REFERENCES content (id) ON DELETE CASCADE,
    FOREIGN KEY (tag_id) REFERENCES tag (id) ON DELETE CASCADE
);

-- The reverse lookup (which content carries a tag) backs tag-scoped filtering.
CREATE INDEX idx_content_tag_tag ON content_tag (tag_id);
