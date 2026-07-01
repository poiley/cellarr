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

/// The coarse live state a download is in, as the client reports it *now*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueDownloadState {
    /// Queued in the client but not started.
    Queued,
    /// Actively downloading.
    Downloading,
    /// Finished; ready to import.
    Completed,
    /// Failed.
    Failed,
}

/// Live progress for one in-flight download, as the client reports it right now.
///
/// Byte/peer fields are `None` when the client omits them (e.g. a magnet whose
/// metadata has not been fetched yet reports no total size). The queue merges this
/// over the stored grab so the FE shows a real percentage / peer count / ETA
/// instead of the release's advertised size and the coarse stored status.
#[derive(Debug, Clone)]
pub struct QueueItemProgress {
    /// Coarse live state (queued / downloading / completed / failed).
    pub state: QueueDownloadState,
    /// Fraction complete in `[0.0, 1.0]`.
    pub progress: f32,
    /// Total size in bytes, when the client knows it. `None`/`0` for a magnet whose
    /// metadata has not been fetched — which is itself the "stuck at 0" signal.
    pub total_bytes: Option<u64>,
    /// Bytes still to download, when known.
    pub size_left: Option<u64>,
    /// Connected peers (seeds + leechers) for torrents, when reported. `Some(0)` on
    /// a stagnant download means "no one to download from".
    pub peers: Option<u32>,
    /// The client's terminal error text on a failed download, when reported.
    pub error: Option<String>,
}

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

    /// Fetch live progress for a download by its client id, for the queue view.
    ///
    /// `Ok(Some(_))` carries the client's live status; `Ok(None)` means the client
    /// has no record of it (dropped/removed) or cannot report progress; `Err` means
    /// the client could not be reached. The queue handler treats any non-`Some`
    /// result as "no live data" and falls back to the stored grab lifecycle, so a
    /// down/slow client never breaks the queue. Defaults to `Ok(None)` (the
    /// offline/test path surfaces only the stored status).
    ///
    /// # Errors
    /// Returns a short, non-secret human string when the client could not be built
    /// or reached.
    async fn progress(&self, download_id: &str) -> Result<Option<QueueItemProgress>, String> {
        let _ = download_id;
        Ok(None)
    }
}
