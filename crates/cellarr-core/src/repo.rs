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

    /// Record the download client's own id for a grab, once it has accepted it.
    async fn set_download_id(&self, id: GrabId, download_id: &str) -> Result<(), Self::Error>;

    /// Advance a grab's lifecycle [`GrabStatus`].
    async fn set_status(&self, id: GrabId, status: GrabStatus) -> Result<(), Self::Error>;
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
