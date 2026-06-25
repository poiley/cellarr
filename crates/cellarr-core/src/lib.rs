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

pub mod auth;
pub mod blocklist;
pub mod config;
pub mod decision;
pub mod delay;
pub mod error;
pub mod history;
pub mod ids;
pub mod importlist;
pub mod media;
pub mod monitor;
pub mod notification;
pub mod parsed;
pub mod pipeline;
pub mod profile;
pub mod release;
pub mod repo;
pub mod tag;
pub mod traits;
pub mod webhook;

pub use auth::{AuthConfig, AuthMethod};
pub use blocklist::{release_key, BlocklistEntry, BlocklistRepository};

pub use config::{
    apply_remote_path_mappings, DownloadClientConfig, ExtraFileImport, ImportPermissions,
    IndexerConfig, IndexerCriteria, MediaManagement, NameTarget, NamingFormats, NotificationConfig,
    RemotePathMapping, RootFolder,
};
pub use decision::{
    Decision, Grab, GrabRequest, GrabStatus, ImportPlan, PlannedMove, RejectReason, Score, Verdict,
};
pub use delay::{resolve_delay_profile, DelayProfile, DelayVerdict, PreferredProtocol};
pub use error::{CoreError, Result};
pub use history::{DecisionLogRecord, HistoryEvent, HistoryRecord};
pub use ids::{
    ContentId, CustomFormatId, DelayProfileId, DownloadClientId, GrabId, IndexerId, LibraryId,
    MediaFileId, PipelineRunId, QualityProfileId, TitleId,
};
pub use importlist::{
    sync_import_list, CleanAction, FetchResult, ImportListConfig, ImportListExclusion,
    ImportListItem, ImportListRepository, ListSource, SyncOutcome,
};
pub use media::{
    ContentKind, ContentMetadata, ContentNode, ContentRef, Coordinates, Library, MediaFile,
    MediaType,
};
pub use monitor::{EpisodeFacts, MonitorOption};
pub use notification::{
    config_accepts, NotificationEvent, NotificationHealth, NotificationMessage,
    NotificationRelease, NotificationSender, NotificationSubject,
};
pub use parsed::{
    Confidence, HdrFormat, ParsedField, ParsedRelease, ProperRepack, Resolution, Source, VideoCodec,
};
pub use pipeline::{is_legal_transition, Stage, Transition, TransitionKind};
pub use profile::{
    condition_matches, custom_format_matches, resolve_quality, Condition, ConditionKind,
    CustomFormat, Quality, QualityDefinition, QualityProfile, QualityRanking,
};
pub use release::{ContentMatch, ParsedCandidate, Protocol, Release, ReleaseType};
pub use repo::{
    ContentRepository, DecisionLogRepository, DeletedContent, GrabRepository, HistoryRepository,
    MediaFileRepository, ProfileRepository,
};
pub use tag::{tag_scope_applies, Tag};
pub use traits::{
    DownloadClient, DownloadState, DownloadStatus, Indexer, MediaModule, MetadataSource,
    NamingTokens, SearchTerms,
};
pub use webhook::{
    WebhookEventType, WebhookFile, WebhookHealth, WebhookPayload, WebhookRelease, WebhookSender,
    WebhookSubject,
};
