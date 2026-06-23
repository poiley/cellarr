//! The `/api/v3` tag store.
//!
//! Sonarr/Radarr expose a small `tag` resource (`{ id, label }`) that the
//! ecosystem round-trips: Overseerr assigns tags to requested items, Bazarr
//! filters on them. cellarr's domain model has no tag concept yet, so rather
//! than force a premature core schema change this provides an in-process,
//! integer-keyed tag store with the same CRUD contract. It is intentionally
//! ephemeral (per running process), which is sufficient for the drop-in surface;
//! a persisted tag domain is a later, additive core change.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// A single tag: a stable integer id and a label, matching the v3 `tag` shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    /// The integer id the ecosystem keys on.
    pub id: u32,
    /// The human-facing label.
    pub label: String,
}

/// A cheap-to-clone, thread-safe tag store. Ids are assigned densely starting
/// at 1 (the *arr convention; tag id 0 is never used).
#[derive(Clone, Default)]
pub struct TagStore {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    next_id: u32,
    tags: BTreeMap<u32, String>,
}

impl TagStore {
    /// All tags, ordered by id.
    #[must_use]
    pub fn list(&self) -> Vec<Tag> {
        let inner = self.inner.lock().expect("tag store poisoned");
        inner
            .tags
            .iter()
            .map(|(id, label)| Tag {
                id: *id,
                label: label.clone(),
            })
            .collect()
    }

    /// One tag by id.
    #[must_use]
    pub fn get(&self, id: u32) -> Option<Tag> {
        let inner = self.inner.lock().expect("tag store poisoned");
        inner.tags.get(&id).map(|label| Tag {
            id,
            label: label.clone(),
        })
    }

    /// Create a tag, returning it with its assigned id. An existing tag with the
    /// same label (case-insensitive) is returned as-is rather than duplicated,
    /// matching the originals' de-duplication.
    pub fn create(&self, label: &str) -> Tag {
        let mut inner = self.inner.lock().expect("tag store poisoned");
        if let Some((id, label)) = inner
            .tags
            .iter()
            .find(|(_, l)| l.eq_ignore_ascii_case(label))
            .map(|(id, l)| (*id, l.clone()))
        {
            return Tag { id, label };
        }
        inner.next_id += 1;
        let id = inner.next_id;
        inner.tags.insert(id, label.to_string());
        Tag {
            id,
            label: label.to_string(),
        }
    }

    /// Update a tag's label. Returns the updated tag, or `None` if absent.
    pub fn update(&self, id: u32, label: &str) -> Option<Tag> {
        let mut inner = self.inner.lock().expect("tag store poisoned");
        if let Some(slot) = inner.tags.get_mut(&id) {
            *slot = label.to_string();
            Some(Tag {
                id,
                label: label.to_string(),
            })
        } else {
            None
        }
    }

    /// Delete a tag. Returns whether it existed.
    pub fn delete(&self, id: u32) -> bool {
        let mut inner = self.inner.lock().expect("tag store poisoned");
        inner.tags.remove(&id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_assigns_dense_ids_from_one() {
        let store = TagStore::default();
        assert_eq!(store.create("anime").id, 1);
        assert_eq!(store.create("4k").id, 2);
    }

    #[test]
    fn create_is_idempotent_on_label() {
        let store = TagStore::default();
        let a = store.create("Anime");
        let b = store.create("anime");
        assert_eq!(a.id, b.id);
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn update_and_delete() {
        let store = TagStore::default();
        let t = store.create("old");
        assert_eq!(store.update(t.id, "new").unwrap().label, "new");
        assert!(store.delete(t.id));
        assert!(!store.delete(t.id));
        assert!(store.get(t.id).is_none());
    }
}
