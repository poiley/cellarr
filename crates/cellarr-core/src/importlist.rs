//! Import lists: pull a curated list of items from an external source and add the
//! monitored ones cellarr does not already have.
//!
//! An **import list** is a source of *what to want*: a Trakt list, a TMDb
//! collection, a Plex watchlist, an IMDb list, … Periodically cellarr fetches the
//! list, resolves each entry to an external id, and **adds the items that are not
//! already in the library** as new monitored content (the originals' "Import
//! List" / "List Sync" feature).
//!
//! ## The safeguard (the #1 library-wipe footgun)
//!
//! The originals support an optional *clean-library* action: items that fall off
//! a list can be unmonitored or removed. That action is only ever safe against a
//! **confirmed-good** fetch. The classic catastrophe is a list source that errors
//! (auth expired, tracker down, rate-limited) and returns *nothing*; if that
//! empty result is treated as "the list is now empty", a clean action wipes the
//! library.
//!
//! cellarr makes that impossible by construction. A fetch returns a
//! [`FetchResult`] that is explicitly either [`FetchResult::Fetched`] (a
//! confirmed-good fetch — the items are exactly what the source returned, even if
//! that is legitimately zero) or [`FetchResult::Failed`] (the fetch could not be
//! completed). [`sync_import_list`]:
//!
//! - **never** performs any removal/clean action on a [`FetchResult::Failed`] —
//!   not even when the failure surfaced as an empty item set;
//! - persists [`last_successful_sync`](ImportListConfig::last_successful_sync) only
//!   on a confirmed-good fetch, so downstream clean logic can require a recent
//!   good sync before acting; and
//! - returns a [`SyncOutcome`] whose [`removable`](SyncOutcome::removable) set is
//!   **always empty** unless the fetch was confirmed-good.
//!
//! These are values + trait seams here (`cellarr-core` does no I/O). The live HTTP
//! list sources live in a crate that owns an HTTP client; the sync orchestration
//! ([`sync_import_list`]) is pure and unit-tested against a mock source.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::media::MediaType;

/// One entry returned by a list source: an item the list says cellarr should
/// want, identified by an external id.
///
/// The id namespace (`tvdb`, `tmdb`, `imdb`, `trakt`, …) plus the id value is the
/// stable identity the sync uses to decide whether the item is already present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportListItem {
    /// The external id namespace this item is keyed in (e.g. `"tvdb"`, `"tmdb"`,
    /// `"imdb"`). Compared case-insensitively.
    pub id_type: String,
    /// The external id value within [`id_type`](Self::id_type).
    pub id_value: String,
    /// The item's title, kept for display/logging and for the added node's title.
    pub title: String,
    /// The item's release/first-air year, when the source provides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    /// The media type the item is (so a TV list never adds movies and vice-versa).
    pub media_type: MediaType,
}

impl ImportListItem {
    /// The normalized `(id_type, id_value)` identity key used to match an item
    /// against the library and the exclusion set. Lowercased so trivial case
    /// differences between sources still match.
    #[must_use]
    pub fn key(&self) -> (String, String) {
        (
            self.id_type.trim().to_ascii_lowercase(),
            self.id_value.trim().to_ascii_lowercase(),
        )
    }
}

/// The result of one list-source fetch — the heart of the empty-vs-failed
/// safeguard.
///
/// A source returns [`Fetched`](Self::Fetched) **only** when it completed a
/// successful round-trip and the carried items are exactly what the list holds
/// (an empty `Vec` here means the list is genuinely empty). Any error — network,
/// auth, parse, rate-limit, an HTTP non-success, a missing credential — must be
/// reported as [`Failed`](Self::Failed), never as `Fetched(vec![])`. The sync
/// path branches on this and treats the two cases completely differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchResult {
    /// A confirmed-good fetch. The items are authoritative (an empty vector means
    /// the list really is empty, which *is* allowed to drive a clean action).
    Fetched(Vec<ImportListItem>),
    /// The fetch could not be completed. The string is a human reason for the
    /// decision log. This **never** drives a removal/clean action.
    Failed(String),
}

impl FetchResult {
    /// Whether this is a confirmed-good fetch (the only case clean actions act on).
    #[must_use]
    pub const fn is_good(&self) -> bool {
        matches!(self, FetchResult::Fetched(_))
    }
}

/// A list source: something that produces an [`ImportListItem`] set on demand.
///
/// This is the abstraction every import-list backend implements — Trakt, TMDb,
/// Plex watchlist, IMDb, a plain Trakt-style RSS, or a test mock. The contract is
/// the safeguard: an implementation **must** return [`FetchResult::Failed`] on
/// any error and reserve [`FetchResult::Fetched`] for a genuine successful
/// round-trip. Returning `Fetched(vec![])` for an error is a contract violation
/// that could wipe a library, so implementations route every error through
/// `Failed`.
#[async_trait]
pub trait ListSource: Send + Sync {
    /// A stable kind string (`"trakt"`, `"tmdb"`, `"plex"`, `"mock"`) for logs and
    /// config round-tripping.
    fn kind(&self) -> &str;

    /// Fetch the current list. Returns [`FetchResult::Failed`] on *any* error
    /// (never a falsely-empty `Fetched`).
    async fn fetch(&self) -> FetchResult;
}

/// A configured import list (the persisted row the user manages + the sync reads).
///
/// Carries the common typed fields the sync reasons about plus a
/// `settings: serde_json::Value` for the source-specific bits core stays ignorant
/// of (a Trakt username/list slug, a TMDb list id, a Plex token), mirroring the
/// other config aggregates (docs/02-data-model.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportListConfig {
    /// List identifier (uuid string).
    pub id: String,
    /// Human-facing name.
    pub name: String,
    /// The source kind (`"trakt"`, `"tmdb"`, `"plex"`, `"imdb"`, `"mock"`),
    /// selecting which [`ListSource`] implementation reads `settings`.
    pub kind: String,
    /// Whether the list is enabled for periodic sync.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The media type items from this list are added as (a list is single-type so
    /// a TV list never injects movies).
    pub media_type: MediaType,
    /// Whether newly-added items are monitored. Almost always `true` (an import
    /// list exists to *want* things), surfaced so a user can stage a list.
    #[serde(default = "default_true")]
    pub monitored: bool,
    /// The clean-library action this list performs for items no longer on the
    /// list. Defaults to [`CleanAction::None`] — the safe default; a destructive
    /// action is strictly opt-in and still gated on a confirmed-good fetch.
    #[serde(default)]
    pub clean_action: CleanAction,
    /// The quality profile new items are added with, when the list pins one;
    /// `None` falls back to the target library's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_profile_id: Option<String>,
    /// The last time this list completed a **confirmed-good** fetch+sync. Set only
    /// on success; a failed fetch never updates it. Clean logic can require this to
    /// be recent before acting.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub last_successful_sync: Option<OffsetDateTime>,
    /// Source-specific settings (Trakt list slug, TMDb list id, Plex token, …).
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// What an import list does with items that have fallen off the list since the
/// last sync. The destructive variants are strictly opt-in and, crucially, are
/// **only ever applied on a confirmed-good fetch** (the safeguard).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanAction {
    /// Never remove or unmonitor anything. The default and the safe choice.
    #[default]
    None,
    /// Unmonitor (but keep) items no longer on the list.
    Unmonitor,
    /// Remove items no longer on the list from the library (metadata only — the
    /// on-disk file removal is a separate, even-more-gated concern).
    Remove,
}

/// The outcome of one [`sync_import_list`] call: what to add, what (if anything)
/// is eligible for a clean action, and whether the fetch was confirmed-good.
///
/// The invariant the safeguard guarantees: when [`fetch_succeeded`](Self::fetch_succeeded)
/// is `false`, [`removable`](Self::removable) is **always empty** and
/// [`addable`](Self::addable) carries nothing — a failed fetch is inert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Whether the underlying fetch was confirmed-good. `false` means the source
    /// errored; the sync made no changes.
    pub fetch_succeeded: bool,
    /// Items on the list that are not already present (nor excluded) — the ones to
    /// add as new monitored content. Empty on a failed fetch.
    pub addable: Vec<ImportListItem>,
    /// Library item keys eligible for the list's [`CleanAction`] because they are
    /// no longer on the list. **Always empty** unless the fetch was confirmed-good
    /// *and* the list opted into a destructive clean action. The caller still
    /// applies the action; this is the gated candidate set.
    pub removable: Vec<(String, String)>,
    /// The clean action the list is configured for (echoed so the caller knows how
    /// to treat [`removable`](Self::removable)).
    pub clean_action: CleanAction,
    /// A human reason, set when the fetch failed (for the decision log).
    pub failure_reason: Option<String>,
}

impl SyncOutcome {
    /// The inert outcome for a failed fetch: nothing to add, nothing removable,
    /// and the failure recorded. This is the *only* outcome a non-good fetch can
    /// produce, which is what makes a failed/empty-because-errored fetch unable to
    /// touch the library.
    #[must_use]
    pub fn failed(clean_action: CleanAction, reason: impl Into<String>) -> Self {
        Self {
            fetch_succeeded: false,
            addable: Vec::new(),
            removable: Vec::new(),
            clean_action,
            failure_reason: Some(reason.into()),
        }
    }
}

/// Run one import-list sync: fetch from `source`, diff against what is already
/// present (`existing`) and excluded (`excluded`), and produce a [`SyncOutcome`].
///
/// This is the pure core of the feature — no I/O beyond the `source.fetch()` the
/// caller's [`ListSource`] performs. It encodes the **empty-vs-failed safeguard**:
///
/// - On [`FetchResult::Failed`] it returns [`SyncOutcome::failed`] immediately:
///   nothing is added, nothing is removable, `fetch_succeeded` is `false`. A
///   caller that performs a clean action sees an empty `removable` and does
///   nothing — a failed (or empty-because-errored) fetch can never wipe the
///   library.
/// - On [`FetchResult::Fetched`] it computes the additions (list items keyed by
///   `(id_type, id_value)` that are neither already present nor excluded) and,
///   *only if* the list opted into a destructive [`CleanAction`], the removable
///   set (present items absent from the confirmed-good list). An empty
///   confirmed-good list legitimately marks everything removable — but only
///   because the fetch was good.
///
/// `existing` and `excluded` are the normalized identity keys (see
/// [`ImportListItem::key`]) the caller resolves from the library and the
/// list-exclusion table.
#[must_use]
pub fn sync_import_list(
    config: &ImportListConfig,
    source_result: FetchResult,
    existing: &[(String, String)],
    excluded: &[(String, String)],
) -> SyncOutcome {
    let items = match source_result {
        FetchResult::Failed(reason) => {
            // The safeguard: a failed fetch is completely inert. We do NOT fall
            // through to any diff/clean path, so neither an explicit error nor an
            // empty-because-errored result can ever drive a removal.
            return SyncOutcome::failed(config.clean_action, reason);
        }
        FetchResult::Fetched(items) => items,
    };

    let existing_set: std::collections::HashSet<(String, String)> =
        existing.iter().cloned().collect();
    let excluded_set: std::collections::HashSet<(String, String)> =
        excluded.iter().cloned().collect();

    // Additions: list items of this list's media type, not already present and not
    // excluded. De-duplicated within the fetch so a list that repeats an entry
    // adds it once.
    let mut seen = std::collections::HashSet::new();
    let mut addable = Vec::new();
    let mut list_keys = std::collections::HashSet::new();
    for item in &items {
        if item.media_type != config.media_type {
            continue;
        }
        let key = item.key();
        list_keys.insert(key.clone());
        if existing_set.contains(&key) || excluded_set.contains(&key) {
            continue;
        }
        if seen.insert(key) {
            addable.push(item.clone());
        }
    }

    // Removable: only when the list opted into a destructive clean action. The set
    // is the present items absent from this confirmed-good list. (It is computed
    // here only because we are on the Fetched branch — a Failed fetch returned
    // above with an empty removable set.)
    let removable = match config.clean_action {
        CleanAction::None => Vec::new(),
        CleanAction::Unmonitor | CleanAction::Remove => existing
            .iter()
            .filter(|k| !list_keys.contains(*k))
            .cloned()
            .collect(),
    };

    SyncOutcome {
        fetch_succeeded: true,
        addable,
        removable,
        clean_action: config.clean_action,
        failure_reason: None,
    }
}

/// One import-list exclusion: an item the user never wants an import list to
/// re-add (the originals' "List Exclusions"). Keyed by external id like a list
/// item so an excluded entry is skipped on every future sync.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportListExclusion {
    /// Exclusion identifier (uuid string).
    pub id: String,
    /// The external id namespace (`"tvdb"`, `"tmdb"`, `"imdb"`).
    pub id_type: String,
    /// The external id value.
    pub id_value: String,
    /// The title, kept for display in the exclusions UI.
    pub title: String,
}

impl ImportListExclusion {
    /// The normalized identity key, matching [`ImportListItem::key`] so an
    /// exclusion suppresses the matching list item.
    #[must_use]
    pub fn key(&self) -> (String, String) {
        (
            self.id_type.trim().to_ascii_lowercase(),
            self.id_value.trim().to_ascii_lowercase(),
        )
    }
}

/// Reads and writes for import-list configuration and exclusions.
///
/// The sync job reads the enabled lists, the API exposes CRUD, and the exclusion
/// set is consulted on every sync so a removed item never reappears.
#[async_trait]
pub trait ImportListRepository: Send + Sync {
    /// The typed error this repository reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Insert or update an import-list configuration (keyed by id).
    async fn upsert(&self, config: &ImportListConfig) -> Result<(), Self::Error>;

    /// Fetch one import-list configuration by id.
    async fn get(&self, id: &str) -> Result<Option<ImportListConfig>, Self::Error>;

    /// All import-list configurations, by name.
    async fn list(&self) -> Result<Vec<ImportListConfig>, Self::Error>;

    /// All *enabled* import-list configurations (the set the sync job runs).
    async fn list_enabled(&self) -> Result<Vec<ImportListConfig>, Self::Error>;

    /// Delete an import-list configuration by id. Idempotent: `true` if a row was
    /// removed.
    async fn delete(&self, id: &str) -> Result<bool, Self::Error>;

    /// Record a confirmed-good sync time for a list (called only on success).
    async fn mark_synced(&self, id: &str, at: OffsetDateTime) -> Result<(), Self::Error>;

    /// Insert or update a list exclusion.
    async fn upsert_exclusion(&self, exclusion: &ImportListExclusion) -> Result<(), Self::Error>;

    /// All list exclusions.
    async fn list_exclusions(&self) -> Result<Vec<ImportListExclusion>, Self::Error>;

    /// Delete a list exclusion by id. Idempotent.
    async fn delete_exclusion(&self, id: &str) -> Result<bool, Self::Error>;
}

/// The serde default for an `enabled`/`monitored` flag: on unless turned off.
const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(clean: CleanAction) -> ImportListConfig {
        ImportListConfig {
            id: "l1".into(),
            name: "My List".into(),
            kind: "mock".into(),
            enabled: true,
            media_type: MediaType::Movie,
            monitored: true,
            clean_action: clean,
            quality_profile_id: None,
            last_successful_sync: None,
            settings: serde_json::Value::Null,
        }
    }

    fn item(id: &str, title: &str) -> ImportListItem {
        ImportListItem {
            id_type: "tmdb".into(),
            id_value: id.into(),
            title: title.into(),
            year: Some(2020),
            media_type: MediaType::Movie,
        }
    }

    fn key(id: &str) -> (String, String) {
        ("tmdb".into(), id.into())
    }

    #[test]
    fn adds_items_not_already_present() {
        let cfg = config(CleanAction::None);
        let fetched = FetchResult::Fetched(vec![item("1", "A"), item("2", "B")]);
        let out = sync_import_list(&cfg, fetched, &[key("1")], &[]);
        assert!(out.fetch_succeeded);
        assert_eq!(out.addable.len(), 1);
        assert_eq!(out.addable[0].id_value, "2");
        assert!(out.removable.is_empty());
    }

    #[test]
    fn excluded_items_are_never_added() {
        let cfg = config(CleanAction::None);
        let fetched = FetchResult::Fetched(vec![item("1", "A"), item("2", "B")]);
        let out = sync_import_list(&cfg, fetched, &[], &[key("2")]);
        assert_eq!(out.addable.len(), 1);
        assert_eq!(out.addable[0].id_value, "1");
    }

    #[test]
    fn case_insensitive_identity_matching() {
        let cfg = config(CleanAction::None);
        let mut weird = item("ABC", "A");
        weird.id_type = "TMDB".into();
        let fetched = FetchResult::Fetched(vec![weird]);
        // Already present under a lowercase key -> not re-added.
        let out = sync_import_list(&cfg, fetched, &[("tmdb".into(), "abc".into())], &[]);
        assert!(out.addable.is_empty());
    }

    #[test]
    fn wrong_media_type_items_skipped() {
        let cfg = config(CleanAction::None);
        let mut tv = item("9", "Show");
        tv.media_type = MediaType::Tv;
        let fetched = FetchResult::Fetched(vec![tv]);
        let out = sync_import_list(&cfg, fetched, &[], &[]);
        assert!(out.addable.is_empty());
    }

    // --- the safeguard ----------------------------------------------------

    #[test]
    fn failed_fetch_adds_nothing_and_removes_nothing_even_with_clean_remove() {
        // A list configured to REMOVE missing items, with a populated library —
        // the exact catastrophe setup. A failed fetch must leave it all intact.
        let cfg = config(CleanAction::Remove);
        let failed = FetchResult::Failed("auth token expired".into());
        let existing = vec![key("1"), key("2"), key("3")];
        let out = sync_import_list(&cfg, failed, &existing, &[]);
        assert!(!out.fetch_succeeded);
        assert!(out.addable.is_empty(), "a failed fetch adds nothing");
        assert!(
            out.removable.is_empty(),
            "a failed fetch must NEVER mark anything removable"
        );
        assert_eq!(out.failure_reason.as_deref(), Some("auth token expired"));
    }

    #[test]
    fn confirmed_empty_list_can_clean_but_a_failed_one_cannot() {
        let existing = vec![key("1"), key("2")];

        // A genuinely empty *confirmed-good* list with clean=Remove DOES mark the
        // present items removable (this is the legitimate clean path).
        let cfg = config(CleanAction::Remove);
        let good_empty = FetchResult::Fetched(vec![]);
        let out_good = sync_import_list(&cfg, good_empty, &existing, &[]);
        assert!(out_good.fetch_succeeded);
        assert_eq!(out_good.removable.len(), 2);

        // The same empty *symptom* from a failure marks nothing removable.
        let failed = FetchResult::Failed("tracker 503".into());
        let out_bad = sync_import_list(&cfg, failed, &existing, &[]);
        assert!(out_bad.removable.is_empty());
    }

    #[test]
    fn clean_none_never_marks_removable_even_on_good_fetch() {
        let cfg = config(CleanAction::None);
        let good = FetchResult::Fetched(vec![item("1", "A")]);
        let out = sync_import_list(&cfg, good, &[key("1"), key("2")], &[]);
        assert!(out.fetch_succeeded);
        assert!(out.removable.is_empty());
    }

    #[test]
    fn good_fetch_marks_only_missing_items_removable() {
        let cfg = config(CleanAction::Unmonitor);
        // List has 1; library has 1,2,3 -> 2 and 3 are missing from the list.
        let good = FetchResult::Fetched(vec![item("1", "A")]);
        let out = sync_import_list(&cfg, good, &[key("1"), key("2"), key("3")], &[]);
        assert_eq!(out.removable.len(), 2);
        assert!(out.removable.contains(&key("2")));
        assert!(out.removable.contains(&key("3")));
        assert_eq!(out.clean_action, CleanAction::Unmonitor);
    }
}
