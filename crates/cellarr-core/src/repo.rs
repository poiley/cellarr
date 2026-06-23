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

use crate::decision::GrabRequest;
use crate::history::{DecisionLogRecord, HistoryRecord};
use crate::ids::{ContentId, GrabId, QualityProfileId};
use crate::media::ContentRef;
use crate::profile::{CustomFormat, QualityProfile};

/// Reads and writes for the structural `content` tree.
#[async_trait]
pub trait ContentRepository: Send + Sync {
    /// The typed error this repository reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Fetch a content node as a [`ContentRef`].
    async fn get(&self, id: ContentId) -> Result<Option<ContentRef>, Self::Error>;

    /// All monitored content nodes that currently lack an acceptable file.
    async fn monitored_missing(&self) -> Result<Vec<ContentRef>, Self::Error>;
}

/// Reads and writes for grabs handed to download clients.
#[async_trait]
pub trait GrabRepository: Send + Sync {
    /// The typed error this repository reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Persist a new grab and return its id.
    async fn create(&self, request: &GrabRequest) -> Result<GrabId, Self::Error>;

    /// Fetch a grab request by id.
    async fn get(&self, id: GrabId) -> Result<Option<GrabRequest>, Self::Error>;
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

    /// All custom formats, used by the decision engine to score releases.
    async fn custom_formats(&self) -> Result<Vec<CustomFormat>, Self::Error>;
}
