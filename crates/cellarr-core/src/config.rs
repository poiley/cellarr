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

/// The serde default for an `enabled` flag: configuration is enabled unless
/// explicitly turned off.
const fn default_true() -> bool {
    true
}
