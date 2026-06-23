//! cellarr-core — the shared heart of cellarr.
//!
//! This crate is the vocabulary every other crate speaks: the domain types, the
//! cross-crate seam traits, and the acquisition pipeline state machine. It
//! contains **no I/O** — no database, no HTTP — only pure types and logic, so it
//! compiles fast and is trivially testable. It is the one crate with no internal
//! dependencies. See `docs/specs/cellarr-core.md`.
//!
//! # The shape of the domain
//!
//! - **Structure is generic; identity is typed.** Movies, TV, music, and books
//!   share one generic structural model ([`ContentRef`], [`Coordinates`]); the
//!   rich, type-specific metadata lives behind a `title_id` and off the
//!   pipeline's path. The pipeline never branches on [`MediaType`] — it
//!   delegates to a [`MediaModule`].
//! - **One pipeline, every type.** [`pipeline::Stage`] and the validated
//!   [`pipeline::Transition`] logic drive every acquisition; each transition
//!   produces a [`history::DecisionLogRecord`] explaining *why*.
//! - **Decisions are explainable.** [`Verdict`] carries its reason so the system
//!   can always answer "why did it grab/reject/upgrade that?".

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod decision;
pub mod error;
pub mod history;
pub mod ids;
pub mod media;
pub mod parsed;
pub mod pipeline;
pub mod profile;
pub mod release;
pub mod repo;
pub mod traits;

pub use decision::{Decision, GrabRequest, ImportPlan, PlannedMove, RejectReason, Score, Verdict};
pub use error::{CoreError, Result};
pub use history::{DecisionLogRecord, HistoryEvent, HistoryRecord};
pub use ids::{
    ContentId, CustomFormatId, DownloadClientId, GrabId, IndexerId, LibraryId, MediaFileId,
    PipelineRunId, QualityProfileId, TitleId,
};
pub use media::{ContentRef, Coordinates, Library, MediaType};
pub use parsed::{
    Confidence, HdrFormat, ParsedField, ParsedRelease, ProperRepack, Resolution, Source, VideoCodec,
};
pub use pipeline::{is_legal_transition, Stage, Transition, TransitionKind};
pub use profile::{
    condition_matches, custom_format_matches, Condition, ConditionKind, CustomFormat,
    QualityDefinition, QualityProfile,
};
pub use release::{ContentMatch, ParsedCandidate, Protocol, Release};
pub use repo::{
    ContentRepository, DecisionLogRepository, GrabRepository, HistoryRepository, ProfileRepository,
};
pub use traits::{
    DownloadClient, DownloadStatus, Indexer, MediaModule, MetadataSource, NamingTokens, SearchTerms,
};
