//! Release profiles: required / ignored / preferred terms matched against a
//! release title, tag-scoped to content.
//!
//! A release profile encodes the Sonarr "Release Profile" behaviour, the term
//! layer that sits alongside custom formats:
//!
//! * **Required** terms — a release must contain *at least one* required term (if
//!   any are configured) or it is rejected. (Sonarr semantics: the "Must Contain"
//!   list; a release lacking every required term is discarded.)
//! * **Ignored** terms — a release containing *any* ignored term is rejected
//!   (the "Must Not Contain" list).
//! * **Preferred** terms — each carries a score; every preferred term that matches
//!   adds its score to the release's total, alongside the custom-format score, so
//!   it influences ranking and selection. A negative score demotes.
//!
//! A term is matched against the release title **case-insensitively**. A term
//! wrapped in slashes (`/pattern/`) is a regex (the Sonarr convention); any other
//! term is a plain substring. Core owns the data model and the plain-substring /
//! term-shape semantics (cheaply unit-testable and regex-free); the *regex*
//! evaluation of `/.../` terms lives in `cellarr-decide`, which owns the
//! fancy-regex dependency. The decision engine combines the per-profile verdicts
//! and the preferred-term score into the grab/reject/score path.

use serde::{Deserialize, Serialize};

use crate::ids::ReleaseProfileId;
use crate::tag::tag_scope_applies;

/// A single preferred term and the score it contributes when it matches the
/// release title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferredTerm {
    /// The term (plain substring, or `/regex/` for a regex), matched
    /// case-insensitively against the release title.
    pub term: String,
    /// The score added to the release's total for each match. May be negative to
    /// demote a matching release.
    pub score: i32,
}

/// A release profile: required / ignored / preferred terms, tag-scoped.
///
/// Mirrors the Sonarr release profile the ecosystem expects. The term lists are
/// matched against the release title (case-insensitive; `/regex/` for a regex,
/// plain substring otherwise). Tag scoping reuses [`tag_scope_applies`]: an
/// untagged profile is global; a tagged profile applies only to content sharing
/// at least one tag id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseProfile {
    /// Profile identifier.
    pub id: ReleaseProfileId,
    /// Human-facing name.
    #[serde(default)]
    pub name: String,
    /// Whether this profile is active. A disabled profile gates and scores
    /// nothing.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The tag ids this profile applies to (empty = global, applies to all
    /// content). Scoped via [`tag_scope_applies`], matching the rest of cellarr's
    /// id-keyed tag handling (indexers, clients, notifications).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<u32>,
    /// Required terms (the "must contain" list). When non-empty, a release must
    /// contain at least one of these terms or it is rejected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
    /// Ignored terms (the "must not contain" list). A release containing any of
    /// these is rejected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored: Vec<String>,
    /// Preferred terms, each with a score added to the release's total on match.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preferred: Vec<PreferredTerm>,
}

impl ReleaseProfile {
    /// A new, empty, enabled, global release profile with the given name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: ReleaseProfileId::new(),
            name: name.into(),
            enabled: true,
            tags: Vec::new(),
            required: Vec::new(),
            ignored: Vec::new(),
            preferred: Vec::new(),
        }
    }

    /// Whether this profile applies to content carrying `content_tags` (by id).
    ///
    /// A tagless profile is global; a tagged profile applies only when it shares
    /// at least one tag id with the content. Reuses [`tag_scope_applies`] so the
    /// scoping rule is single-sourced.
    #[must_use]
    pub fn applies_to(&self, content_tags: &[u32]) -> bool {
        tag_scope_applies(&self.tags, content_tags)
    }
}

/// Whether `term` is a regex term (wrapped in `/.../`), returning the inner
/// pattern; otherwise `None` (it is a plain substring).
///
/// The Sonarr convention: a term surrounded by slashes is a regex. Requires at
/// least one character between the slashes, so a bare `/` or `//` is treated as a
/// plain substring, never an empty regex.
#[must_use]
pub fn regex_term(term: &str) -> Option<&str> {
    let trimmed = term.trim();
    let inner = trimmed.strip_prefix('/')?.strip_suffix('/')?;
    (!inner.is_empty()).then_some(inner)
}

/// Whether a **plain** (non-regex) `term` matches `title`, case-insensitively.
///
/// This is the substring half of the term-matching contract; the regex half
/// (`/pattern/`) is evaluated by `cellarr-decide`, which owns the regex engine.
/// An empty term never matches (it would otherwise match everything).
#[must_use]
pub fn plain_term_matches(term: &str, title: &str) -> bool {
    let needle = term.trim();
    if needle.is_empty() {
        return false;
    }
    title
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

/// The serde default for the `enabled` flag.
const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_term_detects_slash_form() {
        assert_eq!(regex_term("/x264|x265/"), Some("x264|x265"));
        assert_eq!(regex_term("  /hevc/  "), Some("hevc"));
        // Plain terms are not regexes.
        assert_eq!(regex_term("x264"), None);
        assert_eq!(regex_term("/only-open"), None);
        // A bare slash or empty body is a plain substring, never an empty regex.
        assert_eq!(regex_term("/"), None);
        assert_eq!(regex_term("//"), None);
    }

    #[test]
    fn plain_term_matches_case_insensitively() {
        assert!(plain_term_matches(
            "x264",
            "Show.S01E01.1080p.BluRay.X264-GRP"
        ));
        assert!(plain_term_matches("BLURAY", "show.bluray.x264"));
        assert!(!plain_term_matches("x265", "show.x264"));
        // Empty / whitespace term never matches.
        assert!(!plain_term_matches("", "anything"));
        assert!(!plain_term_matches("   ", "anything"));
    }

    #[test]
    fn applies_to_reuses_tag_scope() {
        let mut p = ReleaseProfile::new("anime");
        // Global (tagless) applies to everything.
        assert!(p.applies_to(&[]));
        assert!(p.applies_to(&[1, 2]));
        // Tagged applies only to content sharing a tag id.
        p.tags = vec![2, 5];
        assert!(p.applies_to(&[1, 2]));
        assert!(!p.applies_to(&[1, 3]));
        assert!(!p.applies_to(&[]));
    }
}
