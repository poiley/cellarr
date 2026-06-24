//! The `pending_release` repository: first-seen bookkeeping for delay profiles.
//!
//! A delay profile holds a grabbable release until its protocol's delay has
//! elapsed *since the release was first seen*. This repo persists that "first
//! seen" instant per (content, release-key) so the elapsed window survives across
//! runs and restarts. The upsert remembers the **earliest** sighting — re-seeing
//! the same release never resets its clock — and the row is cleared once the
//! release is grabbed (or the held release vanishes from the indexer).

use cellarr_core::blocklist::release_key;
use cellarr_core::{ContentId, Protocol, Release};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::error::Result;
use crate::writer::WriterHandle;

/// One held release awaiting its delay window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRelease {
    /// The content node the release was held for.
    pub content_id: ContentId,
    /// The stable release key (guid/url/title) the hold is recorded under.
    pub release_key: String,
    /// Unix seconds the release was first seen.
    pub first_seen_at: u64,
    /// The release protocol.
    pub protocol: Protocol,
    /// The advertised title (for the held-releases view).
    pub title: String,
}

/// Reads/writes for delay-profile first-seen bookkeeping.
#[derive(Clone)]
pub struct PendingReleaseRepo {
    pool: SqlitePool,
    writer: WriterHandle,
}

impl PendingReleaseRepo {
    pub(crate) fn new(pool: SqlitePool, writer: WriterHandle) -> Self {
        Self { pool, writer }
    }

    /// Record that `release` was first seen for `content_id` at `now_secs`,
    /// returning the **effective** first-seen instant.
    ///
    /// Idempotent on `(content_id, release_key)`: the first call writes
    /// `now_secs`; later calls keep the earliest stored value (via
    /// `MIN(first_seen_at, now_secs)` on conflict) and return it. So the delay is
    /// always measured from cellarr's first sighting, however many runs observe the
    /// release.
    ///
    /// # Errors
    /// Returns a [`crate::DbError`] on write or read failure.
    pub async fn record_seen(
        &self,
        content_id: ContentId,
        release: &Release,
        now_secs: u64,
    ) -> Result<u64> {
        let cid = content_id.to_string();
        let key = release_key(release);
        let protocol = protocol_str(release.protocol).to_string();
        let title = release.title.clone();
        let now = now_secs as i64;
        let cid_w = cid.clone();
        let key_w = key.clone();
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO pending_release (content_id, release_key, first_seen_at, protocol, title)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(content_id, release_key) DO UPDATE SET
                            first_seen_at = MIN(pending_release.first_seen_at, excluded.first_seen_at)",
                    )
                    .bind(cid_w)
                    .bind(key_w)
                    .bind(now)
                    .bind(protocol)
                    .bind(title)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            })
            .await?;

        // Read back the effective (possibly earlier) first-seen instant.
        let row = sqlx::query(
            "SELECT first_seen_at FROM pending_release WHERE content_id = ?1 AND release_key = ?2",
        )
        .bind(&cid)
        .bind(&key)
        .fetch_one(&self.pool)
        .await?;
        let first: i64 = row.try_get("first_seen_at")?;
        Ok(first.max(0) as u64)
    }

    /// All releases currently held for `content_id`.
    ///
    /// # Errors
    /// Returns a [`crate::DbError`] on read failure.
    pub async fn list_for_content(&self, content_id: ContentId) -> Result<Vec<PendingRelease>> {
        let rows = sqlx::query(
            "SELECT content_id, release_key, first_seen_at, protocol, title
             FROM pending_release WHERE content_id = ?1 ORDER BY first_seen_at ASC",
        )
        .bind(content_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_pending).collect()
    }

    /// Clear the hold for one release (called once it is grabbed).
    ///
    /// Idempotent: returns `true` if a row was removed.
    ///
    /// # Errors
    /// Returns a [`crate::DbError`] on write failure.
    pub async fn clear(&self, content_id: ContentId, release: &Release) -> Result<bool> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let cid = content_id.to_string();
        let key = release_key(release);
        let removed = Arc::new(AtomicBool::new(false));
        let removed_inner = Arc::clone(&removed);
        self.writer
            .submit(move |conn| {
                Box::pin(async move {
                    let result = sqlx::query(
                        "DELETE FROM pending_release WHERE content_id = ?1 AND release_key = ?2",
                    )
                    .bind(cid)
                    .bind(key)
                    .execute(&mut *conn)
                    .await?;
                    removed_inner.store(result.rows_affected() > 0, Ordering::SeqCst);
                    Ok(())
                })
            })
            .await?;
        Ok(removed.load(Ordering::SeqCst))
    }
}

/// The lowercase wire token for a protocol (matches `Protocol`'s serde).
fn protocol_str(p: Protocol) -> &'static str {
    match p {
        Protocol::Torrent => "torrent",
        Protocol::Usenet => "usenet",
    }
}

fn row_to_pending(row: sqlx::sqlite::SqliteRow) -> Result<PendingRelease> {
    let content_id: String = row.try_get("content_id")?;
    let release_key: String = row.try_get("release_key")?;
    let first: i64 = row.try_get("first_seen_at")?;
    let protocol: String = row.try_get("protocol")?;
    let title: String = row.try_get("title")?;
    let protocol = if protocol.eq_ignore_ascii_case("usenet") {
        Protocol::Usenet
    } else {
        Protocol::Torrent
    };
    let uuid = crate::convert::parse_uuid("content_id", &content_id)?;
    Ok(PendingRelease {
        content_id: ContentId::from_uuid(uuid),
        release_key,
        first_seen_at: first.max(0) as u64,
        protocol,
        title,
    })
}
