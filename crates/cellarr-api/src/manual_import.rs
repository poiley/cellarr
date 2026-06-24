//! The manual-import seam the `/api/v3/manualimport` shim reads from.
//!
//! The manual-import screen lets a user point cellarr at a loose folder of media
//! files, see how each one parses and which library item it would land on, then
//! commit the ones they chose — running them through the **same crash-safe import
//! path** an automatic acquisition uses. That is the read-only scan + the
//! crash-safe commit the pipeline runner exposes as
//! [`scan_manual_import`](cellarr_jobs::PipelineRunner::scan_manual_import) and
//! [`import_manual`](cellarr_jobs::PipelineRunner::import_manual).
//!
//! As with [`crate::release_search`], the API crate must not build the live
//! pipeline (the media registry + per-library runner config) itself — that wiring
//! lives in the daemon (`cellarr-cli`). So the shim depends on this thin,
//! object-safe [`ManualImport`] seam; the wiring crate implements it over the real
//! runner and injects it via [`AppState`](crate::state::AppState).
//!
//! # Graceful degradation
//!
//! A scan/commit with no environment ready (no library root / quality profile)
//! returns [`ManualImportOutcome::Unavailable`] with a clear, non-secret reason —
//! **never** an error that would 500 the daemon. The shim renders that as a
//! clearly-flagged empty result so a client degrades rather than breaking,
//! mirroring the release-search seam.

use async_trait::async_trait;

pub use cellarr_jobs::{ManualImportCandidate, ManualImportRequest, ManualImportResult};

/// The outcome of a manual-import **scan** of a loose folder.
///
/// Distinguishing "scanned, found nothing" (`Found(vec![])`) from "no environment
/// configured" ([`Unavailable`](Self::Unavailable)) lets the shim report *why* a
/// scan is empty (no library wired yet) without erroring.
#[derive(Debug, Clone)]
pub enum ManualImportOutcome {
    /// The scan ran and returned these candidates (possibly empty).
    Found(Vec<ManualImportCandidate>),
    /// No environment is configured/ready to scan (no library root / quality
    /// profile). The reason is a short, non-secret human string.
    Unavailable(String),
}

/// The outcome of a manual-import **commit** of the user's chosen files.
#[derive(Debug, Clone)]
pub enum ManualImportCommitOutcome {
    /// The commit ran: `imported` carries each file that landed (renamed, under the
    /// library root, linked to its node) and `errors` carries the per-file failures
    /// that did not abort the rest of the batch.
    Committed {
        /// Files that were imported through the crash-safe path.
        imported: Vec<ManualImportResult>,
        /// Per-file failures (node not found, plan/verify failed) — one string each.
        errors: Vec<String>,
    },
    /// No environment is configured/ready to import (no library root). The reason is
    /// a short, non-secret human string.
    Unavailable(String),
}

/// The object-safe manual-import seam the shim depends on.
///
/// Implemented by the wiring crate over the live
/// [`PipelineRunner`](cellarr_jobs::PipelineRunner); held in
/// [`AppState`](crate::state::AppState) as `Option<Arc<dyn ManualImport>>`. `None`
/// means no pipeline wiring at all (the shim then reports every scan/commit as
/// unavailable — the offline/test default).
#[async_trait]
pub trait ManualImport: Send + Sync {
    /// Scan `folder` (read-only) for loose media files and return the parsed,
    /// identified candidates — moving nothing.
    ///
    /// # Errors
    /// Returns a short human string only for an infrastructure failure the scan
    /// could not recover from (the folder could not be read). "No environment
    /// ready" is **not** an error — it is [`ManualImportOutcome::Unavailable`].
    async fn scan(&self, folder: &str) -> Result<ManualImportOutcome, String>;

    /// Commit the user's chosen `items` through the crash-safe import path.
    ///
    /// # Errors
    /// Returns a short human string only for an infrastructure failure the commit
    /// could not recover from (a repository write failed). A per-file domain
    /// failure is carried in the `errors` of [`ManualImportCommitOutcome::Committed`],
    /// not errored.
    async fn commit(
        &self,
        items: Vec<ManualImportRequest>,
    ) -> Result<ManualImportCommitOutcome, String>;
}
