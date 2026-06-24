//! The import-list **sync** seam the `/api/v3/importlist` shim triggers.
//!
//! An import-list sync fetches each configured list from its source (TMDb, Trakt,
//! Plex, IMDb, a movie collection) and adds the monitored items the library does
//! not already have — under the **empty-vs-failed safeguard**: a failed/empty
//! fetch wipes nothing and never stamps `last_successful_sync`. That orchestration
//! is exactly what [`cellarr_jobs::ImportListSync`] performs.
//!
//! As with [`crate::release_search`] and [`crate::manual_import`], the API crate
//! must not build the live source factory (its HTTP fetcher) or the db-backed sync
//! itself — that wiring lives in the daemon (`cellarr-cli`). So the shim depends on
//! this thin, object-safe [`ImportListSyncRunner`] seam; the wiring crate
//! implements it over the real [`cellarr_jobs::ImportListSync`] and injects it via
//! [`AppState`](crate::state::AppState).
//!
//! # Graceful degradation
//!
//! With no sync wiring at all (the offline/test default — `None` in
//! [`AppState`](crate::state::AppState)), the shim reports the sync trigger as
//! accepted-but-unwired rather than erroring, so a client degrades rather than
//! breaking.

use async_trait::async_trait;

pub use cellarr_jobs::ListSyncReport;

/// The outcome of triggering an import-list sync.
///
/// Distinguishing "ran, here are the per-list reports" from "no sync wiring"
/// lets the shim answer the trigger either way without erroring. Each
/// [`ListSyncReport`] still carries its own `fetch_succeeded` flag, so a list
/// whose source failed is reported (the safeguard) rather than hidden.
#[derive(Debug, Clone)]
pub enum ImportListSyncOutcome {
    /// The sync ran; one report per list synced (empty when a specific list id was
    /// requested but not found, which the handler maps to a 404).
    Ran(Vec<ListSyncReport>),
    /// No sync wiring is configured at all (the offline/test default). The reason
    /// is a short, non-secret human string.
    Unavailable(String),
}

/// The object-safe import-list sync seam the shim depends on.
///
/// Implemented by the wiring crate over the live [`cellarr_jobs::ImportListSync`];
/// held in [`AppState`](crate::state::AppState) as
/// `Option<Arc<dyn ImportListSyncRunner>>`. `None` means no sync wiring at all (the
/// shim then reports every sync trigger as unavailable — the offline/test
/// default).
#[async_trait]
pub trait ImportListSyncRunner: Send + Sync {
    /// Sync every enabled import list.
    ///
    /// # Errors
    /// Returns a short human string only for an infrastructure failure the sync
    /// could not recover from (a persistence failure, a missing target library). A
    /// *source* fetch failure is **not** an error — it is captured per-list in its
    /// [`ListSyncReport`] (with `fetch_succeeded == false` and nothing changed).
    async fn sync_all(&self) -> Result<ImportListSyncOutcome, String>;

    /// Sync exactly one import list by its (cellarr uuid) id.
    ///
    /// # Errors
    /// As [`sync_all`](Self::sync_all). A list id that does not exist is **not** an
    /// error — it is reported as [`ImportListSyncOutcome::Ran`] with an empty report
    /// vector, which the handler maps to a 404.
    async fn sync_one(&self, list_id: &str) -> Result<ImportListSyncOutcome, String>;
}
