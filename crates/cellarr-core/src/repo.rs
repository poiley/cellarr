//! Repository (persistence) seam traits.
//!
//! Consumers depend on these abstractions; `cellarr-db` implements them over
//! SQLite/Postgres. Defining them in core keeps the rest of the system ignorant
//! of the database. Methods are async (the implementations do I/O) and carry an
//! associated `Error` so core stays free of `sqlx`.
//!
//! These trait surfaces are deliberately small and focused; they will grow as
//! the persisted model does, but each stays a single coherent aggregate so an
//! implementation never has to know about an unrelated table.

use async_trait::async_trait;

use crate::decision::{Grab, GrabRequest, GrabStatus};
use crate::history::{DecisionLogRecord, HistoryRecord};
use crate::ids::{ContentId, GrabId, LibraryId, MediaFileId, QualityProfileId};
use crate::media::{ContentNode, ContentRef, MediaFile};
use crate::profile::{CustomFormat, QualityProfile};

/// Reads and writes for the structural `content` tree.
///
/// This is the aggregate `db/media` uses to build and traverse the adjacency
/// list: [`ContentRepository::upsert`] writes a node (parent links included) and
/// [`ContentRepository::children`] walks one level down. The slim
/// [`ContentRepository::get`] / [`ContentRepository::monitored_missing`] reads
/// remain for the pipeline, which only needs [`ContentRef`].
#[async_trait]
pub trait ContentRepository: Send + Sync {
    /// The typed error this repository reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Fetch a content node as a [`ContentRef`].
    async fn get(&self, id: ContentId) -> Result<Option<ContentRef>, Self::Error>;

    /// All monitored content nodes that currently lack an acceptable file.
    async fn monitored_missing(&self) -> Result<Vec<ContentRef>, Self::Error>;

    /// The root nodes (those with no parent) of a library, in stable order — the
    /// series/movie/artist/author entries a library "lists". This is what the
    /// `/api/v3` library list endpoints (`GET /series`, `GET /movie`) and the
    /// native library-content view read, since `monitored_missing` deliberately
    /// excludes container roots (series/season) that are not themselves grabbable.
    async fn roots(&self, library: LibraryId) -> Result<Vec<ContentNode>, Self::Error>;

    /// Insert or update a content node (keyed by [`ContentNode::id`]), so the
    /// adjacency list can be written by `db/media`.
    async fn upsert(&self, node: &ContentNode) -> Result<(), Self::Error>;

    /// The direct children of `parent` in the tree, in stable order.
    async fn children(&self, parent: ContentId) -> Result<Vec<ContentNode>, Self::Error>;

    /// Persist the content-scoped metadata for a node (year/overview/runtime and
    /// the dated facts), written at Identify/Refresh. Replaces any prior row for
    /// the node (upsert), so a re-identify overwrites stale facts.
    async fn set_metadata(
        &self,
        id: ContentId,
        meta: &crate::media::ContentMetadata,
    ) -> Result<(), Self::Error>;

    /// Read the persisted content-scoped metadata for a node, or `None` when the
    /// node has never been identified/refreshed. The detail endpoints and the
    /// calendar read through this.
    async fn metadata(
        &self,
        id: ContentId,
    ) -> Result<Option<crate::media::ContentMetadata>, Self::Error>;

    /// Delete a **movie** node and everything attached to it, transactionally.
    ///
    /// `id` must address a `movie` node; addressing a non-movie (or a missing)
    /// node deletes nothing and returns [`None`] so the caller can 404 the
    /// addressed kind. On success returns the [`DeletedContent`] receipt: the
    /// content ids removed and the on-disk paths of the media files that were
    /// detached — the input the on-disk recycle/unlink step (`cellarr-fs`) needs.
    /// The DB removal and the file removal are deliberately split: the database
    /// record is gone before any byte is touched, and the returned paths are what
    /// the file step then recycles or deletes.
    async fn delete_movie(&self, id: ContentId) -> Result<Option<DeletedContent>, Self::Error>;

    /// Delete a **series** node, its season/episode subtree, and everything
    /// attached to it, transactionally.
    ///
    /// `id` must address a `series` node; addressing a non-series (or a missing)
    /// node deletes nothing and returns [`None`]. On success returns the
    /// [`DeletedContent`] receipt covering the whole subtree, including every
    /// media file path detached anywhere under the series.
    async fn delete_series(&self, id: ContentId) -> Result<Option<DeletedContent>, Self::Error>;
}

/// The receipt of a content delete: what was removed from the database, and the
/// on-disk paths the file step should now recycle or unlink.
///
/// Returning this (rather than touching the filesystem inside the repository)
/// keeps the database layer free of file I/O and lets the caller honor the
/// `deleteFiles` choice: the DB record is always removed; the files are removed
/// only when asked, using [`media_file_paths`](Self::media_file_paths).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeletedContent {
    /// The content node ids removed (the addressed node plus, for a series, its
    /// whole subtree).
    pub content_ids: Vec<ContentId>,
    /// The on-disk paths of the media files detached by the delete (the files
    /// that became orphaned and were removed from `media_file`). These are what
    /// the caller recycles/unlinks when `deleteFiles` is set.
    pub media_file_paths: Vec<String>,
}

/// Reads and writes for `media_file` rows.
///
/// Kept a separate aggregate from [`ContentRepository`]: a file can satisfy
/// several content nodes (multi-episode), so file lifecycle is its own concern.
/// `list_for_content` resolves through the `content_file` link.
#[async_trait]
pub trait MediaFileRepository: Send + Sync {
    /// The typed error this repository reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Persist a new media file.
    async fn create(&self, file: &MediaFile) -> Result<(), Self::Error>;

    /// Fetch a media file by id.
    async fn get(&self, id: MediaFileId) -> Result<Option<MediaFile>, Self::Error>;

    /// Every media file linked to `content` (one node may map to several files,
    /// and one file to several nodes).
    async fn list_for_content(&self, content: ContentId) -> Result<Vec<MediaFile>, Self::Error>;

    /// Delete a media file row by id (the on-disk removal is `cellarr-fs`'s job).
    async fn delete(&self, id: MediaFileId) -> Result<(), Self::Error>;
}

/// Reads and writes for grabs handed to download clients.
#[async_trait]
pub trait GrabRepository: Send + Sync {
    /// The typed error this repository reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Persist a new grab (created with [`GrabStatus::Pending`]) and return its
    /// id.
    async fn create(&self, request: &GrabRequest) -> Result<GrabId, Self::Error>;

    /// Fetch the persisted grab (request + lifecycle) by id.
    async fn get(&self, id: GrabId) -> Result<Option<Grab>, Self::Error>;

    /// Every persisted grab, newest first. Backs the v3 `queue` surface (a queue
    /// item is an in-flight grab) and the queue-management endpoints that resolve a
    /// queue id back to its grab.
    async fn list(&self) -> Result<Vec<Grab>, Self::Error>;

    /// Record the download client's own id for a grab, once it has accepted it.
    async fn set_download_id(&self, id: GrabId, download_id: &str) -> Result<(), Self::Error>;

    /// Advance a grab's lifecycle [`GrabStatus`].
    async fn set_status(&self, id: GrabId, status: GrabStatus) -> Result<(), Self::Error>;

    /// Change the download category a grab is tagged with (the v3 `PUT /queue`
    /// change-category action). Idempotent; a missing id is a no-op.
    async fn set_category(&self, id: GrabId, category: &str) -> Result<(), Self::Error>;

    /// Delete a grab row by id. Used when a queue item is removed (the grab no
    /// longer tracks anything). Idempotent; returns `true` if a row was removed.
    async fn delete(&self, id: GrabId) -> Result<bool, Self::Error>;
}

/// Append-only writes and queries for the history stream.
#[async_trait]
pub trait HistoryRepository: Send + Sync {
    /// The typed error this repository reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Append a history record.
    async fn append(&self, record: &HistoryRecord) -> Result<(), Self::Error>;

    /// All history for a content node, oldest first.
    async fn for_content(&self, id: ContentId) -> Result<Vec<HistoryRecord>, Self::Error>;
}

/// Append-only writes and queries for the decision log.
#[async_trait]
pub trait DecisionLogRepository: Send + Sync {
    /// The typed error this repository reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Append a decision-log record.
    async fn append(&self, record: &DecisionLogRecord) -> Result<(), Self::Error>;
}

/// Reads for quality profiles and custom formats.
#[async_trait]
pub trait ProfileRepository: Send + Sync {
    /// The typed error this repository reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Fetch a quality profile by id.
    async fn get_profile(
        &self,
        id: QualityProfileId,
    ) -> Result<Option<QualityProfile>, Self::Error>;

    /// All quality profiles, ordered by name. Backs the profiles list the UI and
    /// `/api/v3` shim present without first knowing every id.
    async fn list_profiles(&self) -> Result<Vec<QualityProfile>, Self::Error>;

    /// All custom formats, used by the decision engine to score releases.
    async fn custom_formats(&self) -> Result<Vec<CustomFormat>, Self::Error>;
}
