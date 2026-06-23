//! The append-only event and decision streams.
//!
//! cellarr uses a pragmatic event log: authoritative state tables **plus** an
//! immutable [`HistoryRecord`] stream (what happened) and a
//! [`DecisionLogRecord`] stream (why). Every pipeline transition produces one
//! decision-log record value; terminal outcomes also produce history records.
//! These are *values* here — persistence lives in `cellarr-db`.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::decision::Decision;
use crate::ids::{ContentId, GrabId, PipelineRunId};
use crate::pipeline::{Stage, Transition};

/// An immutable record of something that happened to a content node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryRecord {
    /// When the event occurred (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The content node the event concerns.
    pub content_id: ContentId,
    /// The pipeline run that produced the event, for correlation.
    pub run_id: PipelineRunId,
    /// What happened.
    pub event: HistoryEvent,
}

/// The kinds of events recorded in [`HistoryRecord`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum HistoryEvent {
    /// A release was grabbed and handed to a download client.
    Grabbed {
        /// The resulting grab.
        grab_id: GrabId,
        /// The durable release type the grab was made as ([`crate::ReleaseType`]),
        /// recorded so the history stream itself shows whether a full-season pack
        /// was grabbed without re-deriving it from the title. `None` for legacy
        /// records written before this field existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        release_type: Option<crate::ReleaseType>,
    },
    /// A grabbed download completed.
    DownloadCompleted {
        /// The grab that completed.
        grab_id: GrabId,
    },
    /// A download failed and was blocklisted.
    DownloadFailed {
        /// The grab that failed.
        grab_id: GrabId,
        /// Human-readable failure detail.
        detail: String,
    },
    /// Files were imported into the library.
    Imported {
        /// The grab that was imported.
        grab_id: GrabId,
    },
    /// An existing file was upgraded over.
    Upgraded {
        /// The grab that performed the upgrade.
        grab_id: GrabId,
    },
    /// A file was deleted (e.g. replaced by an upgrade).
    Deleted {
        /// Human-readable detail of what and why.
        detail: String,
    },
    /// An import was held for user review.
    HeldForReview {
        /// Why it was held.
        reason: String,
    },
}

/// A record explaining *why* the system acted, appended at each transition.
///
/// Every state transition produces one of these. When a transition reached a
/// verdict (the Decide stage), `decision` carries it; other transitions carry
/// `None` and rely on `transition` + `note`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionLogRecord {
    /// When the transition happened (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The pipeline run that produced this record.
    pub run_id: PipelineRunId,
    /// The stage transition that occurred.
    pub transition: Transition,
    /// The decision reached, when the transition was a Decide outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    /// A short human-readable note (e.g. the failure reason for a failed
    /// transition).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl DecisionLogRecord {
    /// Build a record for a transition that carries no full [`Decision`].
    #[must_use]
    pub fn for_transition(
        run_id: PipelineRunId,
        transition: Transition,
        note: Option<String>,
    ) -> Self {
        Self {
            at: OffsetDateTime::now_utc(),
            run_id,
            transition,
            decision: None,
            note,
        }
    }

    /// The stage the run is in after this record.
    #[must_use]
    pub fn resulting_stage(&self) -> Stage {
        self.transition.to
    }
}
