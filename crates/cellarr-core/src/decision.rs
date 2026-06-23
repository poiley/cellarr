//! Decision outcomes and the work items the pipeline hands downstream.
//!
//! [`Decision`] is the explainable output of the decision engine; [`GrabRequest`]
//! and [`ImportPlan`] are the structured work items for the Grab and Import
//! stages respectively. The arithmetic that produces a [`Decision`] lives in
//! `cellarr-decide`; core owns the shapes so every crate agrees on them and so
//! the decision log can persist them.

use serde::{Deserialize, Serialize};

use crate::ids::{ContentId, DownloadClientId, GrabId, IndexerId, MediaFileId};
use crate::media::ContentRef;
use crate::release::{Release, ReleaseType};

/// The total custom-format score plus the quality rank that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Score {
    /// Quality position in the profile order; higher is better.
    pub quality_rank: u32,
    /// Sum of matching custom-format scores.
    pub custom_format_score: i32,
}

/// The verdict the decision engine reaches for a candidate.
///
/// The variant carries the *reason* so the decision log is self-explanatory and
/// the UI can answer "why did it grab/reject/upgrade that?".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    /// Grab the release; nothing acceptable is on disk yet.
    Grab {
        /// The score that justified the grab.
        score: Score,
    },
    /// Grab the release as an upgrade over an existing file.
    Upgrade {
        /// The file being replaced.
        replacing: MediaFileId,
        /// The score of the file currently on disk.
        from: Score,
        /// The score of the candidate.
        to: Score,
    },
    /// Reject the release, with a machine-readable reason.
    Reject {
        /// Why the candidate was rejected.
        reason: RejectReason,
    },
}

/// Machine-readable rejection reasons (the UI maps these to friendly text).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum RejectReason {
    /// The quality is not allowed by the profile.
    QualityNotAllowed,
    /// The total custom-format score is below the profile minimum.
    BelowMinimumCustomFormatScore,
    /// The release (or its group) is blocklisted.
    Blocklisted,
    /// The size is outside the configured constraints.
    SizeOutOfRange,
    /// A required language is missing.
    LanguageRequirementUnmet,
    /// A file at or above both cutoffs already exists; nothing to do.
    CutoffAlreadyMet,
    /// An existing file is equal or better; no upgrade.
    NotAnUpgrade,
    /// A free-form reason for cases not yet enumerated.
    Other {
        /// Human-readable detail.
        detail: String,
    },
}

/// A decision together with the candidate and content it concerns. This is the
/// value appended to the `decision_log` (see [`crate::history`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// The content node the decision is about.
    pub content_ref: ContentRef,
    /// The candidate considered.
    pub release: Release,
    /// The verdict and its reason.
    pub verdict: Verdict,
}

/// A request to grab a chosen release and hand it to a download client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrabRequest {
    /// The content node(s) this grab is intended to satisfy.
    pub content_ref: ContentRef,
    /// The chosen release.
    pub release: Release,
    /// The indexer the release came from.
    pub indexer_id: IndexerId,
    /// The download client to hand it to.
    pub client_id: DownloadClientId,
    /// The category/label cellarr will tag the download with so it only touches
    /// its own downloads.
    pub category: String,
    /// The durable release type derived from the parse at grab time
    /// ([`ReleaseType::from_parsed`]). Persisted so the reconcile/upgrade path
    /// reads it back instead of re-parsing the title each cycle — the season-pack
    /// re-grab-loop fix. `None` only for legacy grabs written before this field
    /// existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_type: Option<ReleaseType>,
}

/// The lifecycle state of a persisted [`Grab`].
///
/// A grab walks this sequence as the download progresses and is imported. The
/// terminal failure states ([`GrabStatus::Failed`], [`GrabStatus::Blocklisted`])
/// let the pipeline re-search without re-grabbing the same bad release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrabStatus {
    /// Created but not yet handed to a download client.
    Pending,
    /// Accepted by the download client (a `download_id` has been assigned).
    Sent,
    /// Actively downloading.
    Downloading,
    /// Download finished; not yet imported.
    Completed,
    /// Files imported into the library. Terminal (success).
    Imported,
    /// Download or import failed. Terminal (the release may be re-searched).
    Failed,
    /// The release (or its group) was blocklisted so it is never re-grabbed.
    /// Terminal.
    Blocklisted,
}

/// A persisted `grab` row: a release sent to a download client and its lifecycle.
///
/// Where [`GrabRequest`] is the immutable intent the decision engine produces,
/// `Grab` is the row `cellarr-db` stores and mutates as the download progresses:
/// the download client's id is filled in once known, and `status` advances
/// through [`GrabStatus`]. See [`docs/02-data-model.md`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grab {
    /// This grab's identifier.
    pub id: GrabId,
    /// The original request that created the grab.
    pub request: GrabRequest,
    /// The download client's own id for the download, once it has accepted it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_id: Option<String>,
    /// Where the grab is in its lifecycle.
    pub status: GrabStatus,
}

/// One file move within an import plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedMove {
    /// Absolute source path in the download directory.
    pub source_path: String,
    /// Absolute destination path in the library.
    pub destination_path: String,
    /// The content node(s) this file will satisfy (multi-ep => several).
    pub content_ids: Vec<ContentId>,
    /// An existing library file this move would replace, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<MediaFileId>,
    /// The on-disk path of the file being replaced, when it sits at a path
    /// *distinct* from `destination_path`.
    ///
    /// An upgrade that lands at the same path overwrites in place and needs no
    /// separate removal. But a replacement can have a different name (different
    /// quality/codec tokens) or even a different folder, so `replaces`
    /// (a [`MediaFileId`]) is not enough for `cellarr-fs` to delete the old file
    /// — it also needs the concrete path. `None` when there is no replaced file,
    /// or when it is overwritten in place at `destination_path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_path: Option<String>,
    /// Whether the move can be a hardlink (same filesystem) or must be a copy.
    pub hardlink: bool,
}

/// The fully computed Stage output: every move, with nothing mutated yet.
///
/// This is the heart of the stage→verify→commit→log discipline: an `ImportPlan`
/// is pure data describing what *would* happen, produced before any filesystem
/// mutation, so it can be verified and logged before commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportPlan {
    /// The grab being imported.
    pub grab_id: GrabId,
    /// Every planned file move.
    pub moves: Vec<PlannedMove>,
}
