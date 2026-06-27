//! The **pure** plan step: diff a declared section against the tracking ledger.
//!
//! Planning is deliberately separated from applying so it is unit-testable without
//! a database and so the same plan can be *printed* (the `config validate` diff)
//! or *applied* (boot / reconcile) from one source of truth. A plan never touches
//! the DB; it consumes the declared items and the ledger rows config previously
//! wrote, and emits a per-item [`Action`].
//!
//! For each declared section the diff is:
//!
//! - declared name **not** in the ledger        => [`Action::Create`]
//! - in the ledger, **content hash changed**     => [`Action::Update`]
//! - in the ledger, **content hash unchanged**   => [`Action::Unchanged`]
//! - in the ledger but **no longer declared**    => [`Action::Prune`]
//!
//! The ledger only ever contains entities *config* created, so a [`Action::Prune`]
//! can never target a UI-created entity (it has no ledger row). Idempotency falls
//! straight out: re-planning the same file against the ledger it just produced
//! yields all-[`Action::Unchanged`] and zero prunes.

use std::collections::BTreeMap;

use cellarr_db::ManagedEntity;

/// The kind name used both as the ledger `kind` and the section label in diffs.
/// One constant per managed section keeps the strings consistent across plan,
/// apply, and export.
pub mod kind {
    /// Tag vocabulary.
    pub const TAG: &str = "tag";
    /// Per-quality size/title edits.
    pub const QUALITY_DEFINITION: &str = "quality_definition";
    /// Custom formats.
    pub const CUSTOM_FORMAT: &str = "custom_format";
    /// Quality profiles.
    pub const QUALITY_PROFILE: &str = "quality_profile";
    /// Root folders.
    pub const ROOT_FOLDER: &str = "root_folder";
    /// Libraries.
    pub const LIBRARY: &str = "library";
    /// Indexers.
    pub const INDEXER: &str = "indexer";
    /// Download clients.
    pub const DOWNLOAD_CLIENT: &str = "download_client";
    /// Release profiles.
    pub const RELEASE_PROFILE: &str = "release_profile";
    /// Delay profiles.
    pub const DELAY_PROFILE: &str = "delay_profile";
    /// Import lists.
    pub const IMPORT_LIST: &str = "import_list";
    /// Notifications.
    pub const NOTIFICATION: &str = "notification";
    /// Remote-path mappings.
    pub const REMOTE_PATH_MAPPING: &str = "remote_path_mapping";
    /// The naming-formats singleton.
    pub const NAMING: &str = "naming";
    /// The media-management singleton.
    pub const MEDIA_MANAGEMENT: &str = "media_management";
    /// The single-admin auth singleton.
    pub const AUTH: &str = "auth";
}

/// What reconciliation will do with one item of a kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The item is newly declared — create it and record a ledger row.
    Create,
    /// The item is declared and tracked, but its content changed — upsert it and
    /// refresh the ledger hash.
    Update,
    /// The item is declared and tracked with an identical hash — no-op.
    Unchanged,
    /// The item was config-managed and is no longer declared — delete it and drop
    /// its ledger row.
    Prune,
}

impl Action {
    /// A short, stable label for diff output.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Action::Create => "create",
            Action::Update => "update",
            Action::Unchanged => "unchanged",
            Action::Prune => "prune",
        }
    }

    /// Whether applying this action mutates the database (everything but
    /// [`Action::Unchanged`]). Used to detect "pending drift" for the CLI exit code.
    #[must_use]
    pub fn is_change(self) -> bool {
        !matches!(self, Action::Unchanged)
    }
}

/// One planned item: its name, the action, and — for create/update — the content
/// hash to record. For a prune the `entity_id` carries the id to delete (read from
/// the ledger), since the declared item no longer exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    /// The item's stable human name (the reconcile identity).
    pub name: String,
    /// What will happen to it.
    pub action: Action,
    /// The content hash of the declared item (empty for a prune).
    pub content_hash: String,
    /// The ledger `entity_id` for an update/prune (the existing id), or `None` for
    /// a create (the id is assigned at apply time).
    pub entity_id: Option<String>,
}

/// The plan for a single kind: its label and the per-item actions, in a stable
/// order (creates/updates/unchanged in declared order, then prunes by name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindPlan {
    /// The kind label (one of [`kind`]).
    pub kind: String,
    /// The planned items.
    pub items: Vec<PlanItem>,
}

impl KindPlan {
    /// Counts of (create, update, unchanged, prune) for the one-line summary.
    #[must_use]
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for item in &self.items {
            match item.action {
                Action::Create => c.created += 1,
                Action::Update => c.updated += 1,
                Action::Unchanged => c.unchanged += 1,
                Action::Prune => c.pruned += 1,
            }
        }
        c
    }

    /// Whether any item in this kind is a change (drives the drift exit code).
    #[must_use]
    pub fn has_changes(&self) -> bool {
        self.items.iter().any(|i| i.action.is_change())
    }
}

/// Per-kind action counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Items to create.
    pub created: usize,
    /// Items to update (hash changed).
    pub updated: usize,
    /// Items left unchanged.
    pub unchanged: usize,
    /// Items to prune (config-managed, no longer declared).
    pub pruned: usize,
}

/// Diff the `declared` items of one kind (each a `(name, content_hash)` pair, in
/// declared order) against the `ledger` rows config previously wrote for that
/// kind, producing the per-item [`KindPlan`].
///
/// This is the whole diff algorithm, and it is pure: no IO, deterministic output.
/// `declared` carries the *already-hashed* declared items so the caller owns how a
/// spec hashes (see [`super::reconcile`]); the ledger supplies the prior hash and
/// the entity id.
#[must_use]
pub fn diff_kind(kind: &str, declared: &[(String, String)], ledger: &[ManagedEntity]) -> KindPlan {
    // Index the ledger by lowercase name for case-insensitive identity (names are
    // matched case-insensitively everywhere else too).
    let ledger_by_name: BTreeMap<String, &ManagedEntity> = ledger
        .iter()
        .map(|e| (e.name.to_ascii_lowercase(), e))
        .collect();

    let mut items = Vec::new();
    let mut declared_keys = BTreeMap::new();

    for (name, hash) in declared {
        let key = name.to_ascii_lowercase();
        declared_keys.insert(key.clone(), ());
        match ledger_by_name.get(&key) {
            None => items.push(PlanItem {
                name: name.clone(),
                action: Action::Create,
                content_hash: hash.clone(),
                entity_id: None,
            }),
            Some(existing) => {
                let action = if existing.content_hash == *hash {
                    Action::Unchanged
                } else {
                    Action::Update
                };
                items.push(PlanItem {
                    name: name.clone(),
                    action,
                    content_hash: hash.clone(),
                    entity_id: Some(existing.entity_id.clone()),
                });
            }
        }
    }

    // Anything in the ledger that is no longer declared is pruned. Stable order by
    // name so output and apply are deterministic.
    let mut prunes: Vec<&ManagedEntity> = ledger
        .iter()
        .filter(|e| !declared_keys.contains_key(&e.name.to_ascii_lowercase()))
        .collect();
    prunes.sort_by(|a, b| a.name.cmp(&b.name));
    for e in prunes {
        items.push(PlanItem {
            name: e.name.clone(),
            action: Action::Prune,
            content_hash: String::new(),
            entity_id: Some(e.entity_id.clone()),
        });
    }

    KindPlan {
        kind: kind.to_string(),
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger_row(name: &str, id: &str, hash: &str) -> ManagedEntity {
        ManagedEntity {
            kind: "indexer".into(),
            name: name.into(),
            entity_id: id.into(),
            content_hash: hash.into(),
        }
    }

    #[test]
    fn create_when_not_in_ledger() {
        let plan = diff_kind("indexer", &[("a".into(), "h1".into())], &[]);
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].action, Action::Create);
        assert_eq!(plan.items[0].entity_id, None);
    }

    #[test]
    fn unchanged_when_hash_matches() {
        let ledger = [ledger_row("a", "id-a", "h1")];
        let plan = diff_kind("indexer", &[("a".into(), "h1".into())], &ledger);
        assert_eq!(plan.items[0].action, Action::Unchanged);
        assert_eq!(plan.items[0].entity_id.as_deref(), Some("id-a"));
    }

    #[test]
    fn update_when_hash_changed() {
        let ledger = [ledger_row("a", "id-a", "old")];
        let plan = diff_kind("indexer", &[("a".into(), "new".into())], &ledger);
        assert_eq!(plan.items[0].action, Action::Update);
        assert_eq!(plan.items[0].entity_id.as_deref(), Some("id-a"));
    }

    #[test]
    fn prune_when_no_longer_declared() {
        let ledger = [ledger_row("gone", "id-g", "h")];
        let plan = diff_kind("indexer", &[], &ledger);
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].action, Action::Prune);
        assert_eq!(plan.items[0].entity_id.as_deref(), Some("id-g"));
    }

    #[test]
    fn name_matching_is_case_insensitive() {
        let ledger = [ledger_row("Alpha", "id", "h1")];
        let plan = diff_kind("indexer", &[("alpha".into(), "h1".into())], &ledger);
        assert_eq!(plan.items[0].action, Action::Unchanged);
    }

    #[test]
    fn mixed_plan_counts() {
        let ledger = [
            ledger_row("keep", "id-k", "h"),
            ledger_row("change", "id-c", "old"),
            ledger_row("drop", "id-d", "h"),
        ];
        let declared = [
            ("keep".into(), "h".into()),
            ("change".into(), "new".into()),
            ("brand-new".into(), "h2".into()),
        ];
        let plan = diff_kind("indexer", &declared, &ledger);
        let c = plan.counts();
        assert_eq!(c.created, 1);
        assert_eq!(c.updated, 1);
        assert_eq!(c.unchanged, 1);
        assert_eq!(c.pruned, 1);
        assert!(plan.has_changes());
    }

    #[test]
    fn idempotent_plan_has_no_changes() {
        let ledger = [ledger_row("a", "id", "h1"), ledger_row("b", "id2", "h2")];
        let declared = [("a".into(), "h1".into()), ("b".into(), "h2".into())];
        let plan = diff_kind("indexer", &declared, &ledger);
        assert!(!plan.has_changes());
        assert_eq!(plan.counts().unchanged, 2);
    }
}
