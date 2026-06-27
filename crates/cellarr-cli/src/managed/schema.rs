//! The declarative config-as-code schema.
//!
//! [`ManagedConfig`] is the typed shape a managed-config YAML file deserializes
//! into. Its sections mirror the existing `/api/v3` + `cellarr-core` models so the
//! file feels familiar to anyone who has configured Sonarr/Radarr/Prowlarr: an
//! indexer or download client carries the same nested settings the v3 schema
//! exposes (`baseUrl`, `apiKey`, host/port/category, …) as a native YAML map,
//! quality profiles carry allowed qualities + cutoff + custom-format thresholds,
//! and every list item is keyed by a stable human **name** (the reconcile
//! identity).
//!
//! Strictness is deliberate: every struct is `deny_unknown_fields`, so a typo'd
//! key (`apikey` for `apiKey`, `qualtiyProfile`, …) is a hard deserialize error
//! rather than a silently-dropped field that would leave the daemon misconfigured.
//! Cross-reference validation (a library naming a non-existent profile, a profile
//! naming a non-existent custom format) lives in [`super::validate`]; this module
//! owns only the shape.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use cellarr_core::{MediaType, Protocol};

/// The schema version this build understands. A file declaring a different
/// `apiVersion` is rejected with a clear message (forward/backward-incompatible
/// config should fail loudly, never be half-applied).
pub const SUPPORTED_API_VERSION: &str = "cellarr/v1";

/// The full declarative configuration loaded from a managed-config file.
///
/// A **section that is absent** (its field deserializes to `None`) is left
/// entirely untouched by reconciliation — only declared sections are reconciled.
/// This is the distinction between "manage this kind, and these are all of it"
/// (an empty list prunes everything config previously managed) and "do not manage
/// this kind at all" (the field omitted).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManagedConfig {
    /// The schema identifier, e.g. `cellarr/v1`. Required; a mismatch is an error.
    pub api_version: String,
    /// An optional operator-facing revision string, surfaced in logs/diagnostics.
    /// Purely informational — it does not affect reconciliation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// The tag vocabulary (label → reconciled to an integer-keyed tag).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<TagSpec>>,
    /// Per-quality size/title edits, keyed by canonical quality name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_definitions: Option<Vec<QualityDefinitionSpec>>,
    /// Named custom formats (scored condition bundles).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_formats: Option<Vec<CustomFormatSpec>>,
    /// Named quality profiles (allowed qualities + cutoff + CF thresholds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_profiles: Option<Vec<QualityProfileSpec>>,
    /// Root folders libraries import into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_folders: Option<Vec<RootFolderSpec>>,
    /// Libraries (reference root folders + a quality profile by name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub libraries: Option<Vec<LibrarySpec>>,
    /// Indexers (Torznab/Newznab/…), with their nested v3-style settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexers: Option<Vec<IndexerSpec>>,
    /// Download clients (qBittorrent/Transmission/SABnzbd/blackhole/…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_clients: Option<Vec<DownloadClientSpec>>,
    /// Release profiles (required / ignored / preferred terms), tag-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_profiles: Option<Vec<ReleaseProfileSpec>>,
    /// Delay profiles (per-protocol grab delays + preference), tag-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_profiles: Option<Vec<DelayProfileSpec>>,
    /// Import lists (Trakt/TMDb/Plex/…), referencing a quality profile by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_lists: Option<Vec<ImportListSpec>>,
    /// Notification targets (Discord/webhook/…), tag-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<Vec<NotificationSpec>>,
    /// Remote-path mappings (download-client path → cellarr-visible path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_path_mappings: Option<Vec<RemotePathMappingSpec>>,
    /// The library-wide naming formats (a singleton settings document). When
    /// declared, reconciliation sets the whole document; when absent it is left
    /// untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub naming: Option<NamingSpec>,
    /// The library-wide media-management settings (a singleton settings document,
    /// minus naming — naming has its own section). Whole-document set when declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_management: Option<MediaManagementSpec>,
    /// The single-admin web-UI auth configuration (a singleton). Operational DB
    /// state (method + username + a pre-hashed password), so it is managed here;
    /// the password is supplied as an already-hashed PHC string via `${ENV}`
    /// (cellarr never hashes inside the managed engine).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSpec>,
}

/// A managed tag: just its label (the reconcile name *is* the label).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TagSpec {
    /// The tag label (the v3 `tag.label`, deduplicated case-insensitively).
    pub name: String,
}

/// A per-quality edit, mirroring [`cellarr_core::QualityDefinition`]'s editable
/// knobs. `name` is the canonical quality name (e.g. `Bluray-1080p`); the
/// catalogue and rank stay code-owned, so only the editable title/size bounds are
/// declared here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct QualityDefinitionSpec {
    /// The canonical quality name this edit targets (must exist in the catalogue).
    pub name: String,
    /// An optional display title override (defaults to the canonical name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Minimum size, bytes per minute (omitted = no lower bound).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_size_per_min: Option<u64>,
    /// Maximum size, bytes per minute (omitted = no upper bound).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size_per_min: Option<u64>,
    /// Advisory preferred size, bytes per minute (never gates).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_size_per_min: Option<u64>,
}

/// A named custom format: a scored bundle of conditions. The `conditions` reuse
/// [`cellarr_core::Condition`] verbatim (the same `kind`-tagged shape the v3 shim
/// and decision engine speak), so a TRaSH-style definition transcribes directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CustomFormatSpec {
    /// The format's stable human name (the reconcile identity).
    pub name: String,
    /// The score contributed when the format matches (may be negative).
    #[serde(default)]
    pub score: i32,
    /// The conditions that define the format (core's `kind`-tagged Condition).
    #[serde(default)]
    pub conditions: Vec<cellarr_core::Condition>,
}

/// A named quality profile, mirroring the v3 quality-profile write shape and
/// [`cellarr_core::QualityProfile`]. Allowed qualities are named by their
/// canonical quality name (resolved to ranks against the catalogue); custom
/// formats it references are named (resolved to ids against the declared/live
/// formats).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct QualityProfileSpec {
    /// The profile's stable human name (the reconcile identity).
    pub name: String,
    /// The allowed qualities, by canonical name (e.g. `["WEBDL-1080p","Bluray-1080p"]`).
    /// Each must resolve against the (default + edited) quality catalogue.
    #[serde(default)]
    pub qualities: Vec<String>,
    /// Whether upgrades are permitted at all.
    #[serde(default = "default_true")]
    pub upgrades_allowed: bool,
    /// The canonical quality name upgrading stops at. Omitted = the best allowed
    /// quality.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutoff: Option<String>,
    /// Reject anything below this total custom-format score.
    #[serde(default)]
    pub min_custom_format_score: i32,
    /// Stop chasing custom-format score once this total is reached.
    #[serde(default)]
    pub upgrade_until_custom_format_score: i32,
    /// Required language codes; empty means no language requirement.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_languages: Vec<String>,
    /// Custom-format scores this profile references, keyed by custom-format
    /// **name** (mirrors the v3 `formatItems[].score`). Each referenced format must
    /// be declared in `customFormats`. cellarr scores a custom format on the
    /// `CustomFormat` itself, so the score here is a *reference* that must **agree
    /// with that custom format's own `score`** (validation enforces it) — there is
    /// one source of truth and reconcile stays idempotent.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub custom_format_scores: BTreeMap<String, i32>,
}

/// A managed root folder, mirroring [`cellarr_core::RootFolder`]. `name` is the
/// reconcile identity (and the human label); `path` is the on-disk location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RootFolderSpec {
    /// The folder's stable human name (the reconcile identity + label).
    pub name: String,
    /// The absolute path on disk.
    pub path: String,
    /// Whether the folder is enabled for imports.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// A managed library, mirroring [`cellarr_core::Library`]. References root folders
/// and a quality profile **by name** (cross-checked in validation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LibrarySpec {
    /// The library's stable human name (the reconcile identity).
    pub name: String,
    /// The media type every node in the library shares.
    pub media_type: MediaType,
    /// The root folders this library imports into, by root-folder **name**.
    #[serde(default)]
    pub root_folders: Vec<String>,
    /// The default quality profile applied to new items, by profile **name**.
    pub quality_profile: String,
}

/// A managed indexer, mirroring [`cellarr_core::IndexerConfig`] and the v3 indexer
/// shape. The adapter-specific `settings` (baseUrl/apiKey/categories/…) are a
/// native nested YAML map — the same field names the v3 schema's `fields[]` carry,
/// just expressed as a map so secrets like `apiKey` can interpolate `${ENV}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IndexerSpec {
    /// The indexer's stable human name (the reconcile identity).
    pub name: String,
    /// The adapter kind (e.g. `torznab`, `newznab`).
    pub kind: String,
    /// The download protocol this indexer's releases use.
    pub protocol: Protocol,
    /// Whether the indexer is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Priority (lower is preferred, the *arr convention).
    #[serde(default)]
    pub priority: i32,
    /// Per-indexer release-acceptance criteria (seeders/seed targets/flags).
    #[serde(default)]
    pub criteria: cellarr_core::IndexerCriteria,
    /// The tag **names** this indexer is scoped to (resolved to ids; empty =
    /// global). Mirrors the typed-tag scoping the core model carries as ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Adapter-specific settings (base URL, API key, categories, …) as a nested
    /// map — the familiar v3 field names. String values support `${ENV}`.
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// A managed download client, mirroring [`cellarr_core::DownloadClientConfig`] and
/// the v3 download-client shape. As with indexers, `settings`
/// (host/port/username/password/category/…) is a native nested map so credentials
/// interpolate from the environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DownloadClientSpec {
    /// The client's stable human name (the reconcile identity).
    pub name: String,
    /// The adapter kind (e.g. `qbittorrent`, `sabnzbd`, `blackhole`).
    pub kind: String,
    /// The download protocol this client handles.
    pub protocol: Protocol,
    /// Whether the client is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Priority (lower is preferred).
    #[serde(default)]
    pub priority: i32,
    /// The category/label cellarr files its downloads under.
    #[serde(default)]
    pub category: String,
    /// The tag **names** this client is scoped to (resolved to ids; empty = global).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Adapter-specific settings (host, port, credentials, paths, …) as a nested
    /// map. String values support `${ENV}`.
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// A managed release profile, mirroring [`cellarr_core::ReleaseProfile`]. `name`
/// is the reconcile identity; tags are named (resolved to ids), and the term lists
/// transcribe directly (plain substring or `/regex/`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReleaseProfileSpec {
    /// The profile's stable human name (the reconcile identity).
    pub name: String,
    /// Whether this profile is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The tag **names** this profile is scoped to (resolved to ids; empty = global).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Required terms (the "must contain" list).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
    /// Ignored terms (the "must not contain" list).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored: Vec<String>,
    /// Preferred terms, each with a score added on match.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preferred: Vec<cellarr_core::PreferredTerm>,
}

/// A managed delay profile, mirroring [`cellarr_core::DelayProfile`].
///
/// The core [`cellarr_core::DelayProfile`] has no human name (its runtime identity
/// is tags + order). config-as-code keys it by a declared `name` instead — the
/// reconcile identity that the ledger maps to the minted profile id — so an
/// operator can rename/edit a specific delay profile deterministically. The delay
/// profile's own `tags` are opaque label strings (the core model stores them as
/// strings), so they need no cross-reference resolution against the tag vocabulary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DelayProfileSpec {
    /// The profile's stable config name (the reconcile identity; not stored on the
    /// core model, which is name-less — it is the ledger key only).
    pub name: String,
    /// Whether this profile is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Which protocol wins ties when both are available.
    #[serde(default)]
    pub preferred_protocol: cellarr_core::PreferredProtocol,
    /// Minutes to hold a usenet release after it is first seen.
    #[serde(default)]
    pub usenet_delay: u32,
    /// Minutes to hold a torrent release after it is first seen.
    #[serde(default)]
    pub torrent_delay: u32,
    /// Grab a release already at the highest allowed quality immediately.
    #[serde(default)]
    pub bypass_if_highest_quality: bool,
    /// The tag **labels** this profile applies to (opaque strings; empty = the
    /// catch-all default).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Ordering among profiles; lower applies first.
    #[serde(default)]
    pub order: i32,
}

/// A managed import list, mirroring [`cellarr_core::ImportListConfig`]. `name` is
/// the reconcile identity; it references a quality profile **by name** (optional)
/// resolved to an id, and carries source-specific `settings` (with `${ENV}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ImportListSpec {
    /// The list's stable human name (the reconcile identity).
    pub name: String,
    /// The source kind (e.g. `trakt`, `tmdb`, `plex`, `imdb`).
    pub kind: String,
    /// Whether the list is enabled for periodic sync.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The media type items from this list are added as.
    pub media_type: MediaType,
    /// Whether newly-added items are monitored.
    #[serde(default = "default_true")]
    pub monitored: bool,
    /// The clean-library action for items no longer on the list (default `none`).
    #[serde(default)]
    pub clean_action: cellarr_core::importlist::CleanAction,
    /// The quality profile new items are added with, by profile **name** (optional;
    /// falls back to the target library's default). Cross-checked in validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_profile: Option<String>,
    /// Source-specific settings (Trakt list slug, TMDb list id, Plex token, …). A
    /// string value supports `${ENV}` interpolation for secrets.
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// A managed notification target, mirroring [`cellarr_core::NotificationConfig`].
/// `name` is the reconcile identity; tags are named (resolved to ids), and the
/// adapter `settings` carry the webhook URL/credentials (with `${ENV}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NotificationSpec {
    /// The notification's stable human name (the reconcile identity).
    pub name: String,
    /// The adapter kind (e.g. `discord`, `webhook`).
    pub kind: String,
    /// Whether the notification is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The lifecycle events this target fires on (empty = all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_events: Vec<String>,
    /// The tag **names** this notification is scoped to (resolved to ids; empty =
    /// global).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Adapter-specific settings (webhook URL, channel, credentials, …). String
    /// values support `${ENV}`.
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// A managed remote-path mapping, mirroring [`cellarr_core::RemotePathMapping`].
///
/// The core model is keyed by an opaque id, with no human name; config-as-code
/// keys it by a declared `name` (the ledger maps it to the minted id) so the
/// mapping can be edited deterministically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RemotePathMappingSpec {
    /// The mapping's stable config name (the reconcile identity; the ledger key).
    pub name: String,
    /// The download-client host this mapping applies to (empty = any host).
    #[serde(default)]
    pub host: String,
    /// The path prefix as the download client reports it.
    pub remote_path: String,
    /// The path prefix as cellarr sees the same location.
    pub local_path: String,
}

/// The managed naming-formats singleton, mirroring [`cellarr_core::NamingFormats`].
/// Every field defaults to the daemon's built-in *arr-conventional template, so a
/// partial declaration keeps the rest at its default.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NamingSpec {
    /// The TV series folder name (default `{Series Title}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_folder_format: Option<String>,
    /// The TV season folder name (default `Season {Season}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season_folder_format: Option<String>,
    /// The TV episode file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_file_format: Option<String>,
    /// The TV anime episode file name (carries `{Absolute Episode}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anime_episode_file_format: Option<String>,
    /// The movie file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub movie_file_format: Option<String>,
}

/// The managed media-management singleton, mirroring [`cellarr_core::MediaManagement`]
/// minus naming (which has its own [`NamingSpec`] section). Whole-document set when
/// declared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MediaManagementSpec {
    /// The recycle-bin directory deleted media is moved into (absent = unlink).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recycle_bin_path: Option<String>,
    /// The Unix permission policy applied to imported media after commit.
    #[serde(default)]
    pub permissions: cellarr_core::ImportPermissions,
    /// Whether (and which) sibling extra files are imported alongside media.
    #[serde(default)]
    pub extra_files: cellarr_core::ExtraFileImport,
    /// Whether the Kodi/Jellyfin `.nfo` sidecar is written next to imported media.
    #[serde(default = "default_true")]
    pub write_nfo: bool,
}

impl Default for MediaManagementSpec {
    fn default() -> Self {
        let mm = cellarr_core::MediaManagement::default();
        Self {
            recycle_bin_path: mm.recycle_bin_path,
            permissions: mm.permissions,
            extra_files: mm.extra_files,
            write_nfo: mm.write_nfo,
        }
    }
}

/// The managed single-admin auth singleton, mirroring [`cellarr_core::AuthConfig`].
///
/// This is operational DB state (the persisted auth-config row), not figment/env
/// bootstrap config, so it is reconcilable here. The password is **never** hashed
/// inside the managed engine — it is supplied as an already-hashed Argon2 PHC
/// string (typically via a `${ENV}` secret), exactly as the API crate would have
/// stored it. A bare `username` with no `passwordHash` (or vice-versa) leaves the
/// credential unset; validation rejects selecting an enforcing method with no
/// credential (it would lock the operator out).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AuthSpec {
    /// The enforced method (`none`, `forms`, `basic`). Default `none` (open).
    #[serde(default)]
    pub method: cellarr_core::AuthMethod,
    /// The single admin's username (`None` = no credential).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// The admin password **hash** (an Argon2 PHC string), typically a `${ENV}`
    /// secret. Never a plaintext password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
}

/// The serde default for an `enabled`/`upgradesAllowed` flag: on unless turned off.
const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
apiVersion: cellarr/v1
version: "2026-06-26.1"
tags:
  - name: anime
  - name: 4k
qualityDefinitions:
  - name: Bluray-1080p
    title: HD Bluray
    minSizePerMin: 10
    maxSizePerMin: 2000
customFormats:
  - name: x265
    score: -50
    conditions:
      - kind: codec
        codec: x265
qualityProfiles:
  - name: HD
    qualities: [WEBDL-1080p, Bluray-1080p]
    cutoff: Bluray-1080p
    upgradesAllowed: true
    minCustomFormatScore: 0
    customFormatScores:
      x265: -50
rootFolders:
  - name: movies
    path: /data/movies
libraries:
  - name: Movies
    mediaType: movie
    rootFolders: [movies]
    qualityProfile: HD
indexers:
  - name: nzbgeek
    kind: newznab
    protocol: usenet
    priority: 1
    criteria:
      minimumSeeders: 5
    settings:
      baseUrl: https://api.nzbgeek.info
      apiKey: ${NZBGEEK_KEY}
      categories: [5030, 5040]
downloadClients:
  - name: qbit
    kind: qbittorrent
    protocol: torrent
    category: cellarr
    settings:
      host: localhost
      port: 8080
      password: ${QBIT_PASS}
"#;

    #[test]
    fn full_sample_round_trips_through_yaml() {
        let cfg: ManagedConfig = serde_yaml::from_str(SAMPLE).expect("deserialize");
        assert_eq!(cfg.api_version, SUPPORTED_API_VERSION);
        assert_eq!(cfg.version.as_deref(), Some("2026-06-26.1"));
        assert_eq!(cfg.tags.as_ref().unwrap().len(), 2);
        assert_eq!(cfg.indexers.as_ref().unwrap()[0].name, "nzbgeek");
        // Settings preserved as a nested map.
        let ix = &cfg.indexers.as_ref().unwrap()[0];
        assert_eq!(ix.settings["apiKey"], "${NZBGEEK_KEY}");
        assert_eq!(ix.criteria.minimum_seeders, Some(5));

        // Re-serialize and re-parse: structurally identical.
        let yaml = serde_yaml::to_string(&cfg).expect("serialize");
        let back: ManagedConfig = serde_yaml::from_str(&yaml).expect("re-deserialize");
        assert_eq!(cfg, back);
    }

    #[test]
    fn absent_sections_deserialize_to_none() {
        let cfg: ManagedConfig = serde_yaml::from_str("apiVersion: cellarr/v1\n").unwrap();
        assert!(cfg.tags.is_none());
        assert!(cfg.indexers.is_none());
        assert!(cfg.libraries.is_none());
    }

    #[test]
    fn empty_section_is_some_empty_not_none() {
        // An explicit empty list means "manage this kind, declaring none" (prune
        // everything), distinct from omitting the field (do not manage).
        let cfg: ManagedConfig =
            serde_yaml::from_str("apiVersion: cellarr/v1\nindexers: []\n").unwrap();
        assert_eq!(cfg.indexers, Some(Vec::new()));
    }

    #[test]
    fn unknown_field_is_rejected() {
        let err = serde_yaml::from_str::<ManagedConfig>(
            "apiVersion: cellarr/v1\nindexres: []\n", // typo
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "expected unknown-field error, got: {err}"
        );
    }

    const PACK2: &str = r#"
apiVersion: cellarr/v1
releaseProfiles:
  - name: no-x265
    ignored: [x265]
    preferred:
      - term: PROPER
        score: 5
delayProfiles:
  - name: default
    usenetDelay: 30
    preferredProtocol: usenet
importLists:
  - name: trakt
    kind: trakt
    mediaType: movie
    qualityProfile: HD
    settings:
      apiKey: ${K}
notifications:
  - name: discord
    kind: discord
    onEvents: [grab]
    settings:
      webhookUrl: ${W}
remotePathMappings:
  - name: dl
    remotePath: /downloads
    localPath: /data/downloads
naming:
  movieFileFormat: "{Movie Title}.{Extension}"
mediaManagement:
  recycleBinPath: /recycle
  writeNfo: false
auth:
  method: forms
  username: admin
  passwordHash: ${H}
"#;

    #[test]
    fn pack2_sections_round_trip_through_yaml() {
        let cfg: ManagedConfig = serde_yaml::from_str(PACK2).expect("deserialize");
        assert_eq!(cfg.release_profiles.as_ref().unwrap()[0].name, "no-x265");
        assert_eq!(cfg.delay_profiles.as_ref().unwrap()[0].usenet_delay, 30);
        assert_eq!(
            cfg.import_lists.as_ref().unwrap()[0]
                .quality_profile
                .as_deref(),
            Some("HD")
        );
        assert_eq!(cfg.notifications.as_ref().unwrap()[0].kind, "discord");
        assert_eq!(
            cfg.remote_path_mappings.as_ref().unwrap()[0].remote_path,
            "/downloads"
        );
        assert_eq!(
            cfg.naming.as_ref().unwrap().movie_file_format.as_deref(),
            Some("{Movie Title}.{Extension}")
        );
        assert!(!cfg.media_management.as_ref().unwrap().write_nfo);
        assert_eq!(
            cfg.auth.as_ref().unwrap().method,
            cellarr_core::AuthMethod::Forms
        );

        // Structural round-trip.
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        let back: ManagedConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn pack2_unknown_nested_field_is_rejected() {
        let err = serde_yaml::from_str::<ManagedConfig>(
            "apiVersion: cellarr/v1\nnotifications:\n  - name: n\n    kind: discord\n    bogus: 1\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown field"), "got: {err}");
    }

    #[test]
    fn unknown_nested_field_is_rejected() {
        let err = serde_yaml::from_str::<ManagedConfig>(
            "apiVersion: cellarr/v1\nrootFolders:\n  - name: x\n    path: /p\n    bogus: 1\n",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "expected unknown-field error, got: {err}"
        );
    }
}
