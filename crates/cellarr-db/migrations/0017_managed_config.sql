-- Managed-config tracking: the provenance ledger for config-as-code reconciliation.
--
-- cellarr can reconcile its operational configuration from a declarative file
-- (config-as-code): on boot, a typed `ManagedConfig` (loaded from a YAML file a
-- k8s ConfigMap mounts) is diffed against the live DB and applied through the
-- existing repo upserts/deletes. This table is what makes that diff *safe*.
--
-- It records exactly which entities the managed config previously created, keyed
-- by `(kind, name)` — the stable human name the file keys each item by — together
-- with the concrete `entity_id` the repo assigned and a `content_hash` of the
-- declared item. The reconciler reads this ledger to classify each reconcile:
--
--   * a declared (kind, name) absent here  => CREATE
--   * present here, hash changed            => UPDATE
--   * present here, hash unchanged          => UNCHANGED (idempotent no-op)
--   * present here, no longer declared      => PRUNE (delete + drop the row)
--
-- The crucial library-safety property: PRUNE only ever removes entities that
-- appear in THIS table — i.e. entities config created. A UI-created entity is
-- never in the ledger, so a reconcile never deletes it. A whole section absent
-- from the file leaves that kind's rows (and entities) entirely untouched.
--
-- `entity_id` is stored as TEXT so the one ledger holds every kind uniformly: a
-- uuid string (indexer / download client / quality profile / custom format /
-- library / root folder), a small integer (tag), or a stable name
-- (quality definition, whose identity *is* its canonical name).
CREATE TABLE managed_config_entity (
    -- The entity kind this row tracks (e.g. "indexer", "download_client",
    -- "quality_profile", "custom_format", "root_folder", "library", "tag",
    -- "quality_definition"). One namespace per managed section.
    kind         TEXT NOT NULL,
    -- The stable human name the config file keys this item by (within `kind`).
    -- This is the reconcile identity: renaming an item in the file is a prune of
    -- the old name plus a create of the new one.
    name         TEXT NOT NULL,
    -- The concrete id the repo assigned the entity, as text (uuid / integer /
    -- canonical name depending on `kind`). Used to drive the repo upsert/delete.
    entity_id    TEXT NOT NULL,
    -- A stable hash of the declared item's content, so an unchanged declaration
    -- reconciles to a no-op (idempotency) while an edited one is detected as an
    -- update without re-reading the whole live entity.
    content_hash TEXT NOT NULL,
    PRIMARY KEY (kind, name)
);
