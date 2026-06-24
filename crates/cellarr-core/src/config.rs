//! Serializable configuration rows.
//!
//! These are the persisted, user-managed configuration aggregates that the rest
//! of the system reads to know *what* is configured (root folders, indexers,
//! download clients, notifications). Each carries the small set of fields the
//! pipeline reasons about generically, plus a `settings: serde_json::Value` for
//! the adapter-specific bits core deliberately stays ignorant of (an indexer's
//! API key and URL, a download client's host/port/category mapping, a
//! notification target's webhook URL, …). The adapter crate that owns a kind
//! deserializes `settings` into its own typed struct.
//!
//! Keeping the common fields typed and the long tail in one validated JSON column
//! follows the data-model decision in [`docs/02-data-model.md`]: typed where the
//! shape is shared, JSON only for the genuinely open-ended remainder.

use serde::{Deserialize, Serialize};

use crate::ids::{DownloadClientId, IndexerId};
use crate::release::Protocol;

/// Media-management settings: the cross-library file-handling policy.
///
/// This is the small set of file-handling toggles the *arr ecosystem groups
/// under "Media Management". cellarr keeps only the fields its file operations
/// actually reason about; the long tail of cosmetic naming options stays in the
/// `/api/v3` projection, not here.
///
/// The headline field is [`recycle_bin_path`](Self::recycle_bin_path): when set,
/// a content delete that removes media **moves** the files into the recycle bin
/// (preserving their layout relative to the library root) instead of unlinking
/// them, so a mistaken delete is reversible. `None` (the default) unlinks
/// directly, matching the *arr default of an empty recycle-bin path. Mirrors
/// Sonarr/Radarr `recycleBin`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaManagement {
    /// The recycle-bin directory deleted media is moved into instead of being
    /// unlinked. `None`/empty means delete unlinks the file outright (the *arr
    /// default). An absolute path: deleted files land under it, preserving their
    /// path relative to the library root so a restore is unambiguous.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recycle_bin_path: Option<String>,
}

/// A configured root folder a library imports into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootFolder {
    /// Folder identifier.
    pub id: String,
    /// Absolute path on disk.
    pub path: String,
    /// Human-facing label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether the folder is currently enabled for imports.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// A configured indexer (Torznab, Newznab, Cardigann).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexerConfig {
    /// Indexer identifier.
    pub id: IndexerId,
    /// Human-facing name.
    pub name: String,
    /// The adapter kind (e.g. "torznab", "newznab"), selecting which
    /// implementation deserializes `settings`.
    pub kind: String,
    /// The download protocol this indexer's releases use.
    pub protocol: Protocol,
    /// Whether the indexer is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Priority for ordering/tie-breaking (lower is preferred, matching the
    /// *arr convention).
    #[serde(default)]
    pub priority: i32,
    /// Adapter-specific settings (base URL, API key, categories, …).
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// A configured download client (qBittorrent, Deluge, Transmission, SABnzbd,
/// NZBGet).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadClientConfig {
    /// Client identifier.
    pub id: DownloadClientId,
    /// Human-facing name.
    pub name: String,
    /// The adapter kind (e.g. "qbittorrent", "sabnzbd"), selecting which
    /// implementation deserializes `settings`.
    pub kind: String,
    /// The download protocol this client handles.
    pub protocol: Protocol,
    /// Whether the client is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Priority for ordering/tie-breaking (lower is preferred).
    #[serde(default)]
    pub priority: i32,
    /// The category/label cellarr tags its downloads with so it only ever
    /// touches its own downloads.
    pub category: String,
    /// Adapter-specific settings (host, port, credentials, paths, …).
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// A configured notification target (Discord, webhook, email, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationConfig {
    /// Notification identifier.
    pub id: String,
    /// Human-facing name.
    pub name: String,
    /// The adapter kind (e.g. "discord", "webhook"), selecting which
    /// implementation deserializes `settings`.
    pub kind: String,
    /// Whether the notification is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The lifecycle events this target fires on, as stable string keys
    /// (e.g. "grab", "import", "upgrade", "health"). Empty means "all".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_events: Vec<String>,
    /// Adapter-specific settings (webhook URL, channel, credentials, …).
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// A remote-path mapping: how to translate a download client's reported path
/// into a path cellarr can see.
///
/// When the download client and cellarr run on different hosts (or in different
/// containers with different mounts), the client reports a finished download at a
/// path that does not exist from cellarr's vantage point — e.g. the client says
/// `/downloads/Show.S01E01` but cellarr sees that same content at
/// `/data/downloads/Show.S01E01`. A mapping rewrites the client-reported
/// `content_path` from its [`remote_path`](Self::remote_path) prefix to the
/// [`local_path`](Self::local_path) prefix **before Import**.
///
/// This is a *shared* layer applied in one place (the jobs runner), not per
/// adapter: every download client benefits, and the blackhole adapter — which is
/// itself just a folder pair — composes with it cleanly. It mirrors the
/// Sonarr/Radarr `RemotePathMapping` the ecosystem (Recyclarr, UoMi) expects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePathMapping {
    /// Mapping identifier.
    pub id: String,
    /// The download client host the mapping applies to (matched against the
    /// client's configured host). The Sonarr/Radarr convention; cellarr matches
    /// it case-insensitively and treats an empty host as "any host".
    #[serde(default)]
    pub host: String,
    /// The path prefix as the **download client** reports it (e.g.
    /// `/downloads/`).
    pub remote_path: String,
    /// The path prefix as **cellarr** sees the same location (e.g.
    /// `/data/downloads/`).
    pub local_path: String,
}

impl RemotePathMapping {
    /// Apply this mapping to a client-reported `path`, returning the rewritten
    /// path when `path` starts with [`remote_path`](Self::remote_path), or `None`
    /// when it does not match (so the caller can try the next mapping or pass the
    /// path through unchanged).
    ///
    /// Matching is a prefix replacement that respects path boundaries: a
    /// `remote_path` of `/downloads` matches `/downloads/x` and `/downloads`
    /// itself, but not `/downloads-extra/x`. Trailing slashes on either prefix are
    /// normalized so `/downloads` and `/downloads/` behave identically.
    #[must_use]
    pub fn rewrite(&self, path: &str) -> Option<String> {
        let remote = self.remote_path.trim_end_matches('/');
        let local = self.local_path.trim_end_matches('/');
        if remote.is_empty() {
            return None;
        }
        if path == remote {
            return Some(local.to_string());
        }
        let rest = path.strip_prefix(remote)?;
        // Only a boundary match counts: the char after the prefix must be a
        // separator, otherwise `/downloads` would wrongly match `/downloads-x`.
        if rest.starts_with('/') {
            Some(format!("{local}{rest}"))
        } else {
            None
        }
    }

    /// Whether this mapping applies to a download client on `client_host`.
    /// An empty mapping host matches any client; otherwise the comparison is
    /// case-insensitive.
    #[must_use]
    pub fn matches_host(&self, client_host: &str) -> bool {
        self.host.is_empty() || self.host.eq_ignore_ascii_case(client_host)
    }
}

/// Apply the first matching [`RemotePathMapping`] in `mappings` to a
/// client-reported `content_path`, returning the rewritten path (or the original
/// unchanged when none match).
///
/// This is the single shared entry point the pipeline calls before Import so the
/// rewrite lives in exactly one place regardless of which download client
/// produced the path. Mappings are tried in order; the first whose
/// [`host`](RemotePathMapping::host) and [`remote_path`](RemotePathMapping::remote_path)
/// match wins. An empty mapping list (the default) is a no-op.
#[must_use]
pub fn apply_remote_path_mappings(
    mappings: &[RemotePathMapping],
    client_host: &str,
    content_path: &str,
) -> String {
    for mapping in mappings {
        if mapping.matches_host(client_host) {
            if let Some(rewritten) = mapping.rewrite(content_path) {
                return rewritten;
            }
        }
    }
    content_path.to_string()
}

/// The serde default for an `enabled` flag: configuration is enabled unless
/// explicitly turned off.
const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(remote: &str, local: &str) -> RemotePathMapping {
        RemotePathMapping {
            id: "m1".into(),
            host: String::new(),
            remote_path: remote.into(),
            local_path: local.into(),
        }
    }

    #[test]
    fn rewrite_replaces_matching_prefix() {
        let m = mapping("/downloads", "/data/downloads");
        assert_eq!(
            m.rewrite("/downloads/Show.S01E01"),
            Some("/data/downloads/Show.S01E01".into())
        );
    }

    #[test]
    fn rewrite_normalizes_trailing_slash() {
        let m = mapping("/downloads/", "/data/downloads/");
        assert_eq!(m.rewrite("/downloads/x"), Some("/data/downloads/x".into()));
    }

    #[test]
    fn rewrite_respects_path_boundary() {
        let m = mapping("/downloads", "/data/downloads");
        assert_eq!(m.rewrite("/downloads-extra/x"), None);
    }

    #[test]
    fn rewrite_matches_exact_prefix() {
        let m = mapping("/downloads", "/data/downloads");
        assert_eq!(m.rewrite("/downloads"), Some("/data/downloads".into()));
    }

    #[test]
    fn apply_passes_through_unmapped() {
        let maps = [mapping("/downloads", "/data/downloads")];
        assert_eq!(
            apply_remote_path_mappings(&maps, "", "/media/other/file"),
            "/media/other/file"
        );
    }

    #[test]
    fn apply_uses_first_matching() {
        let maps = [
            mapping("/a", "/local/a"),
            mapping("/downloads", "/data/downloads"),
        ];
        assert_eq!(
            apply_remote_path_mappings(&maps, "", "/downloads/x"),
            "/data/downloads/x"
        );
    }

    #[test]
    fn host_scopes_mapping() {
        let mut m = mapping("/downloads", "/data/downloads");
        m.host = "qbit.local".into();
        assert!(m.matches_host("qbit.local"));
        assert!(m.matches_host("QBIT.LOCAL"));
        assert!(!m.matches_host("other.host"));
    }
}
