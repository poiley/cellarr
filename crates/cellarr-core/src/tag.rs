//! The tag domain value and the shared tag-scope predicate.
//!
//! Sonarr/Radarr expose a small `tag` resource (`{ id, label }`) the ecosystem
//! round-trips, and tag-*scope* a content item's delay profile, indexers,
//! download clients, and notifications: a configured item carrying tags applies
//! only to content sharing at least one of those tags, while an item with no tags
//! is global (applies to everything). This module owns the value type and the one
//! predicate that decides "does a tag-scoped thing apply to this content", so the
//! restriction semantics live in exactly one place for indexers, clients, and
//! notifications alike.
//!
//! Tags are matched by integer **id** here (the stable key the ecosystem and the
//! persisted `content_tag` association use). Delay profiles, which historically
//! match on case-insensitive **labels**, keep their own
//! [`applies_to`](crate::DelayProfile::applies_to); the pipeline resolves a
//! node's tag ids to labels for that path.

use serde::{Deserialize, Serialize};

/// A single tag: a stable integer id and a label, matching the v3 `tag` shape.
///
/// Ids are assigned densely from 1 (the *arr convention; id 0 is never used) and
/// labels are deduplicated case-insensitively by the store that mints them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    /// The integer id the ecosystem keys on.
    pub id: u32,
    /// The human-facing label.
    pub label: String,
}

/// Whether a thing scoped to `scope_tags` applies to content carrying
/// `content_tags`, matching by id.
///
/// The Sonarr/Radarr rule: an **untagged** item (`scope_tags` empty) is global
/// and applies to everything; a **tagged** item applies only when it shares at
/// least one tag id with the content. An empty `scope_tags` everywhere reproduces
/// today's behaviour (nothing is restricted), so adding tags never regresses an
/// existing deployment.
#[must_use]
pub fn tag_scope_applies(scope_tags: &[u32], content_tags: &[u32]) -> bool {
    if scope_tags.is_empty() {
        return true;
    }
    scope_tags.iter().any(|t| content_tags.contains(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untagged_scope_is_global() {
        assert!(tag_scope_applies(&[], &[]));
        assert!(tag_scope_applies(&[], &[1, 2]));
    }

    #[test]
    fn tagged_scope_requires_shared_tag() {
        // Shares tag 2 -> applies.
        assert!(tag_scope_applies(&[2, 5], &[1, 2]));
        // No shared tag -> excluded.
        assert!(!tag_scope_applies(&[5, 6], &[1, 2]));
        // Tagged scope vs untagged content -> excluded.
        assert!(!tag_scope_applies(&[1], &[]));
    }
}
