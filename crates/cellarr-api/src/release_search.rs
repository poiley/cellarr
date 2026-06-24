//! The interactive release-search seam the `/api/v3/release` shim reads from.
//!
//! The interactive-search screen needs to show, for one content node, the ranked
//! releases the configured indexers offer — each scored and flagged grabbable or
//! rejected — *without* grabbing any of them. That is exactly the read-only
//! Discover→Parse→Identify→Decide preview the pipeline runner exposes as
//! [`PipelineRunner::preview_releases`](cellarr_jobs::PipelineRunner::preview_releases).
//!
//! But the API crate must not build the live pipeline (indexer set + media
//! registry + per-node runner config) itself — that wiring lives in the daemon
//! (`cellarr-cli`), the same place the [`crate::metadata`] and live job handler
//! are assembled. So the shim depends on this thin, object-safe [`ReleaseSearch`]
//! seam; the wiring crate implements it over the real runner and injects it via
//! [`AppState`](crate::state::AppState).
//!
//! # Graceful degradation
//!
//! A search with no environment ready to run (no enabled indexer / download
//! client / library root) returns [`ReleaseSearchOutcome::Unavailable`] with a
//! clear, non-secret reason — **never** an error that would 500 the daemon. The
//! shim renders that as a clearly-flagged empty result so a client degrades
//! rather than breaking, mirroring the metadata seam.

use async_trait::async_trait;

use cellarr_core::ContentId;

pub use cellarr_jobs::ReleaseCandidate;

/// The outcome of an interactive release search for one content node.
///
/// Distinguishing "ran, found nothing" (`Found(vec![])`) from "no environment
/// configured" ([`Unavailable`](Self::Unavailable)) lets the shim report *why* a
/// search is empty (no indexer/client yet) without erroring.
#[derive(Debug, Clone)]
pub enum ReleaseSearchOutcome {
    /// The search ran and returned these ranked candidates (possibly empty).
    Found(Vec<ReleaseCandidate>),
    /// No environment is configured/ready to run a search. The reason is a short,
    /// non-secret human string (e.g. "no enabled indexer configured").
    Unavailable(String),
}

/// The object-safe interactive release-search seam the shim depends on.
///
/// Implemented by the wiring crate over the live [`PipelineRunner`]; held in
/// [`AppState`](crate::state::AppState) as `Option<Arc<dyn ReleaseSearch>>`.
/// `None` means no pipeline wiring at all (the shim then reports every search as
/// unavailable — the offline/test default).
#[async_trait]
pub trait ReleaseSearch: Send + Sync {
    /// Run the read-only Discover→Decide preview for `content` and return its
    /// ranked candidates.
    ///
    /// # Errors
    /// Returns a short human string only for an infrastructure failure the search
    /// could not recover from (a Discover seam error, a repository read failure).
    /// "No environment ready" is **not** an error — it is
    /// [`ReleaseSearchOutcome::Unavailable`].
    async fn search(&self, content: ContentId) -> Result<ReleaseSearchOutcome, String>;
}

/// The outcome of an **interactive grab**: the user picked a release from the
/// interactive-search screen and asked cellarr to acquire it.
///
/// Unlike a search, a grab builds and drives the download client — so its outcome
/// distinguishes a grab that was queued/imported, one refused for a domain reason
/// (blocklisted, did not identify), and the no-environment case (no enabled
/// download client / library root) reported as
/// [`Unavailable`](Self::Unavailable) so the UI degrades rather than 500ing.
#[derive(Debug, Clone)]
pub enum ReleaseGrabOutcome {
    /// The grab ran to a terminal pipeline outcome. `imported` is true when a file
    /// was downloaded + imported; false when it was queued but not yet importable,
    /// rejected, or held. `detail` is a short human string for the UI toast.
    Grabbed {
        /// Whether a file was downloaded and imported (vs. queued/rejected/held).
        imported: bool,
        /// A short, non-secret human description of the outcome for the UI.
        detail: String,
    },
    /// No environment is configured/ready to grab (no enabled download client /
    /// library root). The reason is a short, non-secret human string.
    Unavailable(String),
}

/// The object-safe **interactive grab** seam the shim depends on.
///
/// Implemented by the wiring crate over the live
/// [`PipelineRunner`](cellarr_jobs::PipelineRunner) — driving the real Grab→Track→
/// Import path for the chosen release; held in
/// [`AppState`](crate::state::AppState) as `Option<Arc<dyn ReleaseGrab>>`. `None`
/// means no pipeline wiring at all (the shim then reports every grab as
/// unavailable — the offline/test default).
#[async_trait]
pub trait ReleaseGrab: Send + Sync {
    /// Grab the release identified by `guid` for `content`, building the download
    /// client and driving Grab→Track→Import.
    ///
    /// # Errors
    /// Returns a short human string only for an infrastructure failure the grab
    /// could not recover from. A release that is blocklisted, does not identify, or
    /// is no longer offered is **not** an error — it is a
    /// [`ReleaseGrabOutcome::Grabbed`] with `imported: false` and an explanatory
    /// `detail`.
    async fn grab(&self, content: ContentId, guid: &str) -> Result<ReleaseGrabOutcome, String>;
}
