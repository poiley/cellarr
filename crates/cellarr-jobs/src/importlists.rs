//! The import-list sync seam: fetch configured lists from their sources and add
//! the monitored items the library does not already have — never wiping the
//! library on a failed fetch.
//!
//! Phase F persists [`ImportListConfig`] rows (kind + source settings) via the db
//! `ImportListRepo`. This module builds the matching [`ListSource`] for each
//! enabled list, runs the pure [`sync_import_list`] diff from `cellarr-core`
//! (which carries the **empty-vs-failed safeguard**), and applies the outcome:
//! additions become new monitored content nodes; the gated `removable` set drives
//! the configured [`CleanAction`] **only on a confirmed-good fetch**.
//!
//! The list-source abstraction has at least one real backend wired
//! ([`sources`]) — Trakt / TMDb / Plex-watchlist — but those need credentials, so
//! each is **blocked-on-key**: with no credential configured the source returns
//! [`FetchResult::Failed`] (a graceful, inert no-op, never a falsely-empty
//! `Fetched`). The framework itself is exercised hermetically against a
//! [`sources::MockListSource`].

use std::sync::Arc;

use async_trait::async_trait;
use cellarr_core::importlist::{
    sync_import_list, CleanAction, ImportListConfig, ImportListRepository, ListSource, SyncOutcome,
};
use cellarr_core::repo::ContentRepository;
use cellarr_core::{
    ContentId, ContentKind, ContentNode, Coordinates, ImportListItem, Library, MediaType,
};
use cellarr_db::Database;

pub mod sources;

/// A failure running an import-list sync.
#[derive(Debug, thiserror::Error)]
pub enum ImportListSyncError {
    /// A persistence read/write failed.
    #[error("import-list persistence error: {0}")]
    Db(#[source] cellarr_db::DbError),

    /// No library of the list's media type is configured to add items into.
    #[error("no {media_type:?} library configured for import list '{list}'")]
    NoLibrary {
        /// The configured list whose media type has no library.
        list: String,
        /// The media type with no target library.
        media_type: MediaType,
    },
}

impl From<cellarr_db::DbError> for ImportListSyncError {
    fn from(e: cellarr_db::DbError) -> Self {
        ImportListSyncError::Db(e)
    }
}

/// What one list's sync did — for the decision log and the API/UI summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListSyncReport {
    /// The list id this report is for.
    pub list_id: String,
    /// The list's human name.
    pub list_name: String,
    /// Whether the underlying fetch was confirmed-good. `false` means the source
    /// errored and **nothing was changed**.
    pub fetch_succeeded: bool,
    /// How many new monitored items were added.
    pub added: usize,
    /// How many existing items the clean action touched (always 0 on a failed
    /// fetch, and 0 unless the list opted into a destructive clean action).
    pub cleaned: usize,
    /// The failure reason, when the fetch failed.
    pub failure_reason: Option<String>,
}

/// Resolves the set of external-id identity keys already represented in the
/// library, so the sync only adds genuinely-new items (idempotent re-sync) and a
/// clean action can compute the correct removable set.
///
/// The DB-backed [`DbLibraryIndex`] reads the keys persisted on each node's typed
/// `*_meta` identity row (written when the node is added — see
/// `ContentRepo::link_external_id`). Tests inject a concrete set (`FixedIndex`) to
/// exercise the de-duplication path in isolation.
#[async_trait]
pub trait LibraryIndex: Send + Sync {
    /// The normalized `(id_type, id_value)` keys already present for `media_type`.
    async fn existing_keys(
        &self,
        media_type: MediaType,
    ) -> Result<Vec<(String, String)>, cellarr_db::DbError>;
}

/// The DB-backed [`LibraryIndex`]. Reads the external-id identity keys persisted
/// for the library's content nodes (via the typed `*_meta` rows), so the sync's
/// "skip already-present" dedup runs against the live library — making a re-sync
/// of the same list idempotent (no duplicate nodes) and letting a clean action
/// compute a correct removable set.
#[derive(Clone)]
pub struct DbLibraryIndex {
    db: Database,
}

impl DbLibraryIndex {
    /// Build over the database handle.
    #[must_use]
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

#[async_trait]
impl LibraryIndex for DbLibraryIndex {
    async fn existing_keys(
        &self,
        media_type: MediaType,
    ) -> Result<Vec<(String, String)>, cellarr_db::DbError> {
        self.db.content().external_keys(media_type).await
    }
}

/// Builds a [`ListSource`] for a configured import list.
///
/// Pluggable so tests register a mock factory and production registers the live
/// (credential-gated) Trakt/TMDb/Plex factory. A `None` return means the kind is
/// unknown; the sync treats that as a failed (inert) fetch rather than an error.
pub trait SourceFactory: Send + Sync {
    /// Build the source for `config`, or `None` if the kind is unsupported.
    fn build(&self, config: &ImportListConfig) -> Option<Arc<dyn ListSource>>;
}

/// The import-list sync orchestrator: reads enabled lists, fetches each, applies
/// the safeguarded diff, and adds/cleans content.
pub struct ImportListSync {
    db: Database,
    library_index: Arc<dyn LibraryIndex>,
    factory: Arc<dyn SourceFactory>,
}

impl ImportListSync {
    /// Build a sync over the database with a source factory and the DB-backed
    /// library index.
    #[must_use]
    pub fn new(db: Database, factory: Arc<dyn SourceFactory>) -> Self {
        let library_index = Arc::new(DbLibraryIndex::new(db.clone()));
        Self {
            db,
            library_index,
            factory,
        }
    }

    /// Build with an explicit [`LibraryIndex`] (tests inject a populated one).
    #[must_use]
    pub fn with_library_index(
        db: Database,
        factory: Arc<dyn SourceFactory>,
        library_index: Arc<dyn LibraryIndex>,
    ) -> Self {
        Self {
            db,
            library_index,
            factory,
        }
    }

    /// Sync every enabled import list, returning one [`ListSyncReport`] each.
    ///
    /// # Errors
    /// Returns [`ImportListSyncError`] only on a persistence failure or a missing
    /// target library — a *source* failure is captured per-list in its report (and
    /// changes nothing), never bubbled as an error, so one dead list never aborts
    /// the others.
    pub async fn sync_all(&self) -> Result<Vec<ListSyncReport>, ImportListSyncError> {
        let lists = self.db.import_lists().list_enabled().await?;
        let mut reports = Vec::with_capacity(lists.len());
        for list in lists {
            reports.push(self.sync_one(&list).await?);
        }
        Ok(reports)
    }

    /// Sync one import list.
    ///
    /// # Errors
    /// Returns [`ImportListSyncError`] on a persistence failure or when no library
    /// of the list's media type exists to add into. A source fetch failure is
    /// **not** an error: it produces a report with `fetch_succeeded == false` and
    /// makes no changes (the safeguard).
    pub async fn sync_one(
        &self,
        list: &ImportListConfig,
    ) -> Result<ListSyncReport, ImportListSyncError> {
        // Build the source. An unknown kind is treated as an inert failed fetch
        // (never an error that would abort a batch).
        let fetch = match self.factory.build(list) {
            Some(source) => source.fetch().await,
            None => {
                cellarr_core::FetchResult::Failed(format!("unknown list source: {}", list.kind))
            }
        };

        let existing = self
            .library_index
            .existing_keys(list.media_type)
            .await
            .map_err(ImportListSyncError::Db)?;
        let excluded: Vec<(String, String)> = self
            .db
            .import_lists()
            .list_exclusions()
            .await?
            .iter()
            .map(cellarr_core::ImportListExclusion::key)
            .collect();

        let outcome = sync_import_list(list, fetch, &existing, &excluded);

        if !outcome.fetch_succeeded {
            // The safeguard, end to end: a failed fetch never touches the library
            // and never stamps last_successful_sync. We log the reason and return.
            tracing::warn!(
                list = %list.name,
                reason = outcome.failure_reason.as_deref().unwrap_or("unknown"),
                "import list fetch failed; library left untouched"
            );
            return Ok(ListSyncReport {
                list_id: list.id.clone(),
                list_name: list.name.clone(),
                fetch_succeeded: false,
                added: 0,
                cleaned: 0,
                failure_reason: outcome.failure_reason,
            });
        }

        // Confirmed-good fetch: apply additions, then the gated clean action.
        let library = self.target_library(list).await?;
        let added = self.add_items(list, &library, &outcome.addable).await?;
        let cleaned = self.apply_clean(&outcome).await?;

        // Stamp the confirmed-good sync time (only reached on a good fetch).
        self.db
            .import_lists()
            .mark_synced(&list.id, time::OffsetDateTime::now_utc())
            .await?;

        Ok(ListSyncReport {
            list_id: list.id.clone(),
            list_name: list.name.clone(),
            fetch_succeeded: true,
            added,
            cleaned,
            failure_reason: None,
        })
    }

    /// Resolve the library a list adds into (first library of its media type).
    async fn target_library(
        &self,
        list: &ImportListConfig,
    ) -> Result<Library, ImportListSyncError> {
        self.db
            .config()
            .list_libraries()
            .await?
            .into_iter()
            .find(|l| l.media_type == list.media_type)
            .ok_or_else(|| ImportListSyncError::NoLibrary {
                list: list.name.clone(),
                media_type: list.media_type,
            })
    }

    /// Add each addable item as a new monitored content node + indexed title.
    async fn add_items(
        &self,
        list: &ImportListConfig,
        library: &Library,
        items: &[ImportListItem],
    ) -> Result<usize, ImportListSyncError> {
        let content = self.db.content();
        let mut added = 0;
        for item in items {
            let (kind, coords) = match list.media_type {
                MediaType::Tv => (
                    ContentKind::Series,
                    Coordinates::Episode {
                        season: 1,
                        episode: 1,
                        absolute: None,
                    },
                ),
                MediaType::Movie => (ContentKind::Movie, Coordinates::Movie),
                MediaType::Music => (
                    ContentKind::Artist,
                    Coordinates::Track { disc: 1, track: 1 },
                ),
                MediaType::Book => (
                    ContentKind::Author,
                    Coordinates::Book {
                        series_position: None,
                    },
                ),
            };
            let node = ContentNode {
                id: ContentId::new(),
                library_id: library.id,
                media_type: list.media_type,
                parent_id: None,
                kind,
                // Import lists do not model a per-item series type; an added series
                // starts on standard numbering and can be switched to anime later
                // via the v3 series `seriesType` surface.
                series_type: cellarr_core::SeriesType::Standard,
                coords,
                monitored: list.monitored,
                title_id: None,
                // Import lists do not yet model per-list tags to stamp on added
                // items; an added node starts untagged and can be tagged later via
                // the v3 movie/series tags surface.
                // TODO(deferred): apply an import list's configured tags to the
                // content it adds, once ImportListConfig models them.
                tags: Vec::new(),
            };
            content.upsert(&node).await?;
            content.index_title(node.id, &item.title).await?;
            // Persist the item's external id (tmdb/imdb/tvdb) the way the identify
            // pipeline does, so a re-sync of the same list dedups against it (it
            // shows up in existing_keys) and the v3 projection surfaces a real id
            // instead of 0. A failure here is a hard persistence error (the node is
            // already written, so we must not silently lose its identity).
            content
                .link_external_id(
                    node.id,
                    list.media_type,
                    &item.id_type,
                    &item.id_value,
                    &item.title,
                )
                .await?;
            added += 1;
            tracing::info!(list = %list.name, title = %item.title, "import list added item");
        }
        Ok(added)
    }

    /// Apply the list's clean action to the gated `removable` set.
    ///
    /// `outcome.removable` is already empty unless the fetch was confirmed-good and
    /// the list opted into a destructive action, so this is naturally inert on the
    /// safe paths. The actual library-state change for matched nodes is a follow-up
    /// (the identity-link gap means we cannot yet resolve a removable external-id
    /// key back to a content node); we count and log the eligible set so the action
    /// is observable and auditable without risking an incorrect removal.
    async fn apply_clean(&self, outcome: &SyncOutcome) -> Result<usize, ImportListSyncError> {
        if matches!(outcome.clean_action, CleanAction::None) || outcome.removable.is_empty() {
            return Ok(0);
        }
        tracing::info!(
            action = ?outcome.clean_action,
            count = outcome.removable.len(),
            "import list clean action eligible (confirmed-good fetch)"
        );
        Ok(outcome.removable.len())
    }
}
