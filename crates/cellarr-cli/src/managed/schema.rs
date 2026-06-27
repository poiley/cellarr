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
