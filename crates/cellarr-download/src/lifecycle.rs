//! Shared lifecycle types every adapter speaks.
//!
//! `cellarr-core`'s [`DownloadState`] is the four-state summary
//! (`Queued`/`Downloading`/`Completed`/`Failed`) the pipeline's state machine
//! branches on. The adapters compute the same detail core now carries on
//! [`cellarr_core::DownloadStatus`] (on-disk path, progress, seed ratio/time),
//! plus the client `category` that core does not model. They build that detail
//! into [`DownloadProgress`] and project it into the core status via
//! [`DownloadProgress::to_core_status`], so the adapter view and the core view
//! never disagree.
//!
//! [`DownloadProgress::state`] is the single source of truth for the core enum.

use cellarr_core::{DownloadState, DownloadStatus};

/// The detailed state of one tracked download.
///
/// This is what an adapter knows after polling the client. It projects into the
/// core [`DownloadStatus`] via [`to_core_status`](Self::to_core_status) for the
/// pipeline; the only field core does not carry is [`category`](Self::category),
/// which scopes cellarr to its own downloads.
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadProgress {
    /// The coarse lifecycle state the pipeline branches on.
    pub state: DownloadState,
    /// Fraction complete in `[0.0, 1.0]`.
    pub progress: f64,
    /// The on-disk path of the downloaded content, once the client reports one.
    ///
    /// For Usenet this is only meaningful **after** repair/unpack; an adapter
    /// reports [`DownloadState::Completed`] only once the content sits at a
    /// final, importable path (see `docs/06-integrations.md`).
    pub content_path: Option<String>,
    /// Seed ratio for torrents (uploaded / downloaded), when known. `None` for
    /// Usenet, which does not seed.
    pub ratio: Option<f64>,
    /// Seeding time in seconds for torrents, when known. `None` for Usenet.
    pub seeding_time_secs: Option<u64>,
    /// Connected peers (seeds + leechers) for torrents, when the client reports
    /// it. `None` for Usenet / clients that omit it; the stall detector treats
    /// `None` as unknown and only a reported `Some(0)` as a no-peers signal.
    pub peers: Option<u32>,
    /// The client's terminal error text, when it reports one. Carried through to
    /// the core status so a failed download names *why* it failed.
    pub error_string: Option<String>,
    /// The category/label the client has the download filed under. Used to scope
    /// cellarr to its own downloads; a foreign category means "not ours".
    pub category: Option<String>,
}

impl DownloadProgress {
    /// Project this adapter view into the core [`DownloadStatus`] the
    /// [`DownloadClient`](cellarr_core::DownloadClient) trait returns.
    ///
    /// Carries through the path, progress, and seed signals the executor needs;
    /// the `category` is dropped because core does not model it (it is an
    /// adapter-internal scoping concern). `progress` is narrowed from `f64` to the
    /// core `f32` — a fraction in `[0, 1]` loses no meaningful precision.
    #[must_use]
    pub fn to_core_status(&self) -> DownloadStatus {
        DownloadStatus {
            state: self.state,
            progress: self.progress as f32,
            content_path: self.content_path.clone(),
            ratio: self.ratio.map(|r| r as f32),
            seeding_time_secs: self.seeding_time_secs,
            peers: self.peers,
            error_string: self.error_string.clone(),
        }
    }

    /// Whether this download belongs to cellarr, i.e. is filed under
    /// `expected_category`.
    ///
    /// Category scoping is a hard rule: cellarr only ever touches downloads it
    /// tagged, so a status/remove against a foreign category is refused by the
    /// caller (see `docs/06-integrations.md`).
    #[must_use]
    pub fn is_in_category(&self, expected_category: &str) -> bool {
        self.category.as_deref() == Some(expected_category)
    }
}

/// The policy that gates torrent removal on seed ratio / seeding time.
///
/// A torrent is only removable once it has *either* met the ratio target *or*
/// seeded for the minimum time (the *arr convention: satisfy one, not both).
/// Usenet downloads do not seed, so removal is unconditional for them and this
/// policy is not consulted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RemovePolicy {
    /// Remove once the seed ratio reaches this value, if set.
    pub min_ratio: Option<f64>,
    /// Remove once seeding time reaches this many seconds, if set.
    pub min_seeding_time_secs: Option<u64>,
    /// Whether to delete the downloaded data along with the torrent.
    pub delete_data: bool,
}

impl RemovePolicy {
    /// A policy that removes immediately and deletes data (used for Usenet and
    /// for explicit user-driven removal).
    #[must_use]
    pub const fn immediate(delete_data: bool) -> Self {
        Self {
            min_ratio: None,
            min_seeding_time_secs: None,
            delete_data,
        }
    }

    /// Whether `progress` satisfies this removal policy.
    ///
    /// With no ratio and no time target the policy is unconditional. Otherwise
    /// **either** target being met is sufficient. A torrent that reports neither
    /// ratio nor seeding time yet (freshly completed) is not removable under a
    /// gated policy.
    #[must_use]
    pub fn is_satisfied_by(&self, progress: &DownloadProgress) -> bool {
        if self.min_ratio.is_none() && self.min_seeding_time_secs.is_none() {
            return true;
        }
        let ratio_met = match (self.min_ratio, progress.ratio) {
            (Some(target), Some(actual)) => actual >= target,
            _ => false,
        };
        let time_met = match (self.min_seeding_time_secs, progress.seeding_time_secs) {
            (Some(target), Some(actual)) => actual >= target,
            _ => false,
        };
        ratio_met || time_met
    }
}
