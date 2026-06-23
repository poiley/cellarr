//! The failed-download blocklist.
//!
//! When a download fails (or a user manually marks a grab failed), the release is
//! added to a **blocklist** so the decision/grab path never re-grabs the same bad
//! release. On the next search the pipeline consults the blocklist and skips a
//! blocklisted candidate, trying the next release instead (the `download-failed →
//! blocklist + re-search` failure transition in `docs/03-pipeline.md`).
//!
//! A blocklist entry is keyed by a stable **release key** derived from the
//! release's identity (its indexer GUID when present, else its download URL, else
//! its title), scoped to the content node it was grabbed for. Matching is by that
//! key so a re-search that re-discovers the identical release recognizes it.
//!
//! These are values + a repository seam here; persistence lives in `cellarr-db`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::ContentId;
use crate::release::Release;

/// The stable identity a blocklist entry is keyed on.
///
/// Derived from a [`Release`] so a re-discovered identical release maps to the
/// same key: prefer the indexer GUID (the release's unique id on the indexer),
/// fall back to the download URL/magnet, and finally the title. Normalized to a
/// lowercase, trimmed string so trivial case/whitespace differences still match.
#[must_use]
pub fn release_key(release: &Release) -> String {
    let raw = release
        .guid
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or(Some(release.download_url.as_str()))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(release.title.as_str());
    raw.trim().to_ascii_lowercase()
}

/// One blocklisted release: the key, the content it was grabbed for, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlocklistEntry {
    /// The blocklist row id (uuid string).
    pub id: String,
    /// The content node the failed grab was for. Scoping by content means
    /// blocklisting a bad release for one item never hides it for an unrelated
    /// item that happens to share a title fragment.
    pub content_id: ContentId,
    /// The stable [`release_key`] this entry matches on.
    pub release_key: String,
    /// The release title, kept for display in the blocklist UI / `/api/v3` list.
    pub title: String,
    /// The indexer the release came from, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexer: Option<String>,
    /// The protocol of the blocklisted release, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// Why it was blocklisted (the failure detail).
    pub reason: String,
    /// When it was blocklisted (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub blocklisted_at: OffsetDateTime,
}

impl BlocklistEntry {
    /// Build an entry for `release` grabbed for `content_id`, blocklisted at `at`
    /// because of `reason`. The id is freshly generated and the key derived from
    /// the release.
    #[must_use]
    pub fn from_release(
        content_id: ContentId,
        release: &Release,
        reason: impl Into<String>,
        at: OffsetDateTime,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            content_id,
            release_key: release_key(release),
            title: release.title.clone(),
            indexer: None,
            protocol: Some(
                match release.protocol {
                    crate::release::Protocol::Torrent => "torrent",
                    crate::release::Protocol::Usenet => "usenet",
                }
                .to_string(),
            ),
            reason: reason.into(),
            blocklisted_at: at,
        }
    }
}

/// Reads and writes for the failed-download blocklist.
///
/// The decision/grab path calls [`is_blocklisted`](BlocklistRepository::is_blocklisted)
/// before grabbing a candidate; the failure path calls
/// [`add`](BlocklistRepository::add) when a download fails. The `/api/v3/blocklist`
/// surface uses [`list`](BlocklistRepository::list) and
/// [`remove`](BlocklistRepository::remove) (the manual "clear" that lets a release
/// be re-grabbed).
#[async_trait]
pub trait BlocklistRepository: Send + Sync {
    /// The typed error this repository reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Add a release to the blocklist. Idempotent on `(content_id, release_key)`:
    /// re-blocklisting the same release for the same content refreshes the row
    /// rather than duplicating it.
    async fn add(&self, entry: &BlocklistEntry) -> Result<(), Self::Error>;

    /// Whether `release` is blocklisted for `content_id` (matched by
    /// [`release_key`]).
    async fn is_blocklisted(
        &self,
        content_id: ContentId,
        release: &Release,
    ) -> Result<bool, Self::Error>;

    /// All blocklist entries, newest first (the `/api/v3/blocklist` list).
    async fn list(&self) -> Result<Vec<BlocklistEntry>, Self::Error>;

    /// Remove one blocklist entry by id (the manual "clear"). Idempotent: returns
    /// `true` if a row was removed, `false` if no such entry existed.
    async fn remove(&self, id: &str) -> Result<bool, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::IndexerId;
    use crate::release::Protocol;

    fn release(title: &str, guid: Option<&str>, url: &str) -> Release {
        Release {
            indexer_id: IndexerId::new(),
            title: title.into(),
            download_url: url.into(),
            guid: guid.map(String::from),
            protocol: Protocol::Torrent,
            size: None,
            seeders: None,
            indexer_flags: Vec::new(),
        }
    }

    #[test]
    fn release_key_prefers_guid_then_url_then_title() {
        let with_guid = release("Title", Some("GUID-123"), "magnet:x");
        assert_eq!(release_key(&with_guid), "guid-123");

        let no_guid = release("Title", None, "magnet:ABC");
        assert_eq!(release_key(&no_guid), "magnet:abc");

        let bare = release("Some Title", Some("  "), "");
        assert_eq!(release_key(&bare), "some title");
    }

    #[test]
    fn identical_release_maps_to_same_key() {
        let a = release("X", Some("g1"), "u1");
        let b = release("X", Some("G1"), "u2");
        assert_eq!(release_key(&a), release_key(&b));
    }
}
