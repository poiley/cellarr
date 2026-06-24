//! The queue-management seam the `/api/v3/queue` shim drives.
//!
//! The v3 queue is cellarr's set of in-flight grabs (a queue item *is* a grab
//! handed to a download client). The queue-management endpoints mirror
//! Sonarr/Radarr:
//!
//! - `DELETE /queue/{id}?removeFromClient=&blocklist=` — drop a queue item,
//!   optionally telling the download client to remove the download (and its data)
//!   and optionally blocklisting the release so it is never re-grabbed.
//! - `PUT /queue/{id}` (change category) — retag a queued download's category.
//! - grab-from-queue (manual import of a completed-but-unmatched download) reuses
//!   the [`crate::manual_import`] commit path.
//!
//! The DB-side work (list grabs, delete a grab row, blocklist a release) is done
//! by the shim directly over the persistence layer it already holds. The **only**
//! piece that needs the live download client is "remove from client" — building a
//! download-client adapter is daemon wiring (`cellarr-cli`), not the API crate. So
//! that one action goes through this thin, object-safe [`QueueDownloadClient`]
//! seam, injected via [`AppState`](crate::state::AppState). `None` (the
//! offline/test default) means the client cannot be reached, so a
//! `removeFromClient` request is reported as not-performed rather than erroring —
//! the queue row is still removed (the queue is cellarr's own state).

use async_trait::async_trait;

/// The object-safe "remove a download from its client" seam the queue-remove path
/// depends on.
///
/// Implemented by the wiring crate over the configured download client; held in
/// [`AppState`](crate::state::AppState) as `Option<Arc<dyn QueueDownloadClient>>`.
/// `None` means no client wiring at all — a `removeFromClient` request then
/// degrades to "not performed" (the cellarr queue row is still removed) rather
/// than erroring.
#[async_trait]
pub trait QueueDownloadClient: Send + Sync {
    /// Remove the download identified by the client's own `download_id` from the
    /// download client, deleting its on-disk data when `delete_data` is set.
    ///
    /// # Errors
    /// Returns a short, non-secret human string when the client could not be built
    /// or refused the removal. The queue handler logs it and still removes the
    /// cellarr queue row (the queue is cellarr's own state; a client that is down
    /// must not strand a queue item).
    async fn remove(&self, download_id: &str, delete_data: bool) -> Result<(), String>;
}
