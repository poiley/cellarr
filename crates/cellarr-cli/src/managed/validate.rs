//! Semantic validation of a [`ManagedConfig`] beyond what the type system catches.
//!
//! The schema's `deny_unknown_fields` guarantees the *shape*; this module
//! guarantees the *references resolve*. The checks, all of which the task requires
//! to fail with a clear message:
//!
//! - **Unique names** within each section — the name is the reconcile identity, so
//!   two items sharing a name is ambiguous (which one wins a prune?).
//! - **Library → root folder / quality profile.** A library naming a root folder
//!   or quality profile not declared in the file is rejected.
//! - **Quality profile → quality / custom format.** A profile naming an allowed
//!   quality (or a `cutoff`) that is not a real quality, or scoring a custom format
//!   not declared in the file, is rejected.
//! - **Quality definition → catalogue.** A per-quality edit naming a quality that
//!   does not exist in the code-owned catalogue is rejected.
//! - **Indexer / download client → tag.** A tag name referenced for scoping that is
//!   not declared in the file is rejected.
//!
//! References resolve against the **declared** file (the config-as-code contract is
//! self-contained: the file fully describes the managed surface, so a reference
//! must be declared alongside what uses it). The exception is a profile's allowed
//! quality, which resolves against the code-owned quality catalogue (qualities are
//! not user-declared entities).

use std::collections::BTreeSet;

use cellarr_core::QualityRanking;

use crate::managed::error::ManagedError;
use crate::managed::schema::ManagedConfig;

/// Validate every cross-reference and uniqueness constraint in `config`.
///
/// # Errors
/// Returns [`ManagedError::Validation`] describing the first failed check.
pub fn validate(config: &ManagedConfig) -> Result<(), ManagedError> {
    // The known quality names (code-owned catalogue), case-insensitive set.
    let catalogue: BTreeSet<String> = QualityRanking::default()
        .qualities
        .iter()
        .map(|q| q.name.to_ascii_lowercase())
        .collect();

    // --- Per-section unique names -----------------------------------------
    if let Some(tags) = &config.tags {
        unique_names("tag", tags.iter().map(|t| &t.name))?;
    }
    if let Some(defs) = &config.quality_definitions {
        unique_names("qualityDefinition", defs.iter().map(|d| &d.name))?;
    }
    if let Some(cfs) = &config.custom_formats {
        unique_names("customFormat", cfs.iter().map(|c| &c.name))?;
    }
    if let Some(ps) = &config.quality_profiles {
        unique_names("qualityProfile", ps.iter().map(|p| &p.name))?;
    }
    if let Some(rfs) = &config.root_folders {
        unique_names("rootFolder", rfs.iter().map(|r| &r.name))?;
    }
    if let Some(libs) = &config.libraries {
        unique_names("library", libs.iter().map(|l| &l.name))?;
    }
    if let Some(ixs) = &config.indexers {
        unique_names("indexer", ixs.iter().map(|i| &i.name))?;
    }
    if let Some(dcs) = &config.download_clients {
        unique_names("downloadClient", dcs.iter().map(|d| &d.name))?;
    }
    if let Some(rps) = &config.release_profiles {
        unique_names("releaseProfile", rps.iter().map(|r| &r.name))?;
    }
    if let Some(dps) = &config.delay_profiles {
        unique_names("delayProfile", dps.iter().map(|d| &d.name))?;
    }
    if let Some(lists) = &config.import_lists {
        unique_names("importList", lists.iter().map(|l| &l.name))?;
    }
    if let Some(notifs) = &config.notifications {
        unique_names("notification", notifs.iter().map(|n| &n.name))?;
    }
    if let Some(maps) = &config.remote_path_mappings {
        unique_names("remotePathMapping", maps.iter().map(|m| &m.name))?;
    }

    // --- Declared name sets for cross-reference resolution ----------------
    let declared_profiles = name_set(config.quality_profiles.as_deref(), |p| &p.name);
    let declared_root_folders = name_set(config.root_folders.as_deref(), |r| &r.name);
    let declared_formats = name_set(config.custom_formats.as_deref(), |c| &c.name);
    let declared_tags = name_set(config.tags.as_deref(), |t| &t.name);

    // --- Quality definitions reference the catalogue ----------------------
    if let Some(defs) = &config.quality_definitions {
        for d in defs {
            if !catalogue.contains(&d.name.to_ascii_lowercase()) {
                return Err(ManagedError::Validation(format!(
                    "qualityDefinition `{}` is not a known quality (it must match a \
                     canonical quality name in the catalogue)",
                    d.name
                )));
            }
        }
    }

    // --- Quality profiles reference qualities + custom formats ------------
    if let Some(profiles) = &config.quality_profiles {
        for p in profiles {
            if p.qualities.is_empty() {
                return Err(ManagedError::Validation(format!(
                    "qualityProfile `{}` declares no qualities; at least one allowed \
                     quality is required",
                    p.name
                )));
            }
            for q in &p.qualities {
                if !catalogue.contains(&q.to_ascii_lowercase()) {
                    return Err(ManagedError::Validation(format!(
                        "qualityProfile `{}` references unknown quality `{}`",
                        p.name, q
                    )));
                }
            }
            if let Some(cutoff) = &p.cutoff {
                if !catalogue.contains(&cutoff.to_ascii_lowercase()) {
                    return Err(ManagedError::Validation(format!(
                        "qualityProfile `{}` cutoff references unknown quality `{}`",
                        p.name, cutoff
                    )));
                }
                // The cutoff must be among the profile's own allowed qualities.
                if !p.qualities.iter().any(|q| q.eq_ignore_ascii_case(cutoff)) {
                    return Err(ManagedError::Validation(format!(
                        "qualityProfile `{}` cutoff `{}` is not in its allowed qualities",
                        p.name, cutoff
                    )));
                }
            }
            for (cf_name, score) in &p.custom_format_scores {
                if !contains_ci(&declared_formats, cf_name) {
                    return Err(ManagedError::Validation(format!(
                        "qualityProfile `{}` scores custom format `{}`, which is not \
                         declared in customFormats",
                        p.name, cf_name
                    )));
                }
                // The custom-format score is authoritative on the custom format
                // itself (cellarr's model); a profile's `customFormatScores` is a
                // reference that must agree with the declared CF score, so the file
                // has a single source of truth and reconcile stays idempotent.
                if let Some(cf) = config
                    .custom_formats
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .find(|c| c.name.eq_ignore_ascii_case(cf_name))
                {
                    if cf.score != *score {
                        return Err(ManagedError::Validation(format!(
                            "qualityProfile `{}` scores custom format `{}` as {}, but the \
                             customFormats section declares its score as {} (the custom \
                             format's own score is authoritative — make them match)",
                            p.name, cf_name, score, cf.score
                        )));
                    }
                }
            }
        }
    }

    // --- Libraries reference root folders + a quality profile -------------
    if let Some(libraries) = &config.libraries {
        for lib in libraries {
            if !contains_ci(&declared_profiles, &lib.quality_profile) {
                return Err(ManagedError::Validation(format!(
                    "library `{}` references quality profile `{}`, which is not \
                     declared in qualityProfiles",
                    lib.name, lib.quality_profile
                )));
            }
            if lib.root_folders.is_empty() {
                return Err(ManagedError::Validation(format!(
                    "library `{}` declares no root folders; at least one is required",
                    lib.name
                )));
            }
            for rf in &lib.root_folders {
                if !contains_ci(&declared_root_folders, rf) {
                    return Err(ManagedError::Validation(format!(
                        "library `{}` references root folder `{}`, which is not \
                         declared in rootFolders",
                        lib.name, rf
                    )));
                }
            }
        }
    }

    // --- Indexer / download-client tag scoping references tags ------------
    if let Some(ixs) = &config.indexers {
        for ix in ixs {
            for tag in &ix.tags {
                if !contains_ci(&declared_tags, tag) {
                    return Err(ManagedError::Validation(format!(
                        "indexer `{}` is scoped to tag `{}`, which is not declared in tags",
                        ix.name, tag
                    )));
                }
            }
            // A Cardigann indexer needs a definition source. An inline definition is
            // parsed now, so broken YAML is caught at config-apply time rather than at
            // the first search; a `definitionFile` is read at runtime on the target
            // host, so only its presence is checked here.
            if ix.kind.eq_ignore_ascii_case("cardigann") {
                let inline = ix
                    .settings
                    .get("definition")
                    .or_else(|| ix.settings.get("definitionYaml"))
                    .and_then(|v| v.as_str());
                let has_file = ix
                    .settings
                    .get("definitionFile")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.is_empty());
                match inline {
                    Some(y) => {
                        if let Err(e) = cellarr_indexers::Definition::from_yaml(y) {
                            return Err(ManagedError::Validation(format!(
                                "indexer `{}` has an invalid cardigann definition: {e}",
                                ix.name
                            )));
                        }
                    }
                    None if has_file => {}
                    None => {
                        return Err(ManagedError::Validation(format!(
                            "indexer `{}` is kind `cardigann` but has no `settings.definition` \
                             or `settings.definitionFile`",
                            ix.name
                        )))
                    }
                }
            }
        }
    }
    if let Some(dcs) = &config.download_clients {
        for dc in dcs {
            for tag in &dc.tags {
                if !contains_ci(&declared_tags, tag) {
                    return Err(ManagedError::Validation(format!(
                        "downloadClient `{}` is scoped to tag `{}`, which is not declared in tags",
                        dc.name, tag
                    )));
                }
            }
        }
    }

    // --- Release-profile tag scoping references tags ----------------------
    if let Some(rps) = &config.release_profiles {
        for rp in rps {
            for tag in &rp.tags {
                if !contains_ci(&declared_tags, tag) {
                    return Err(ManagedError::Validation(format!(
                        "releaseProfile `{}` is scoped to tag `{}`, which is not declared in tags",
                        rp.name, tag
                    )));
                }
            }
        }
    }

    // --- Notification tag scoping references tags -------------------------
    if let Some(notifs) = &config.notifications {
        for n in notifs {
            for tag in &n.tags {
                if !contains_ci(&declared_tags, tag) {
                    return Err(ManagedError::Validation(format!(
                        "notification `{}` is scoped to tag `{}`, which is not declared in tags",
                        n.name, tag
                    )));
                }
            }
        }
    }

    // Note: delay-profile `tags` are opaque label strings on the core model (not
    // resolved against the tag vocabulary), so they are intentionally NOT
    // cross-checked against `tags` — declaring a delay profile scoped to a label is
    // valid whether or not a matching tag entity is declared.

    // --- Import lists reference a quality profile -------------------------
    if let Some(lists) = &config.import_lists {
        for list in lists {
            if let Some(profile) = &list.quality_profile {
                if !contains_ci(&declared_profiles, profile) {
                    return Err(ManagedError::Validation(format!(
                        "importList `{}` references quality profile `{}`, which is not \
                         declared in qualityProfiles",
                        list.name, profile
                    )));
                }
            }
        }
    }

    // --- Remote-path mappings need a non-empty remote/local path ----------
    if let Some(maps) = &config.remote_path_mappings {
        for m in maps {
            if m.remote_path.trim().is_empty() || m.local_path.trim().is_empty() {
                return Err(ManagedError::Validation(format!(
                    "remotePathMapping `{}` must declare a non-empty remotePath and localPath",
                    m.name
                )));
            }
        }
    }

    // --- Auth must not lock the operator out ------------------------------
    if let Some(auth) = &config.auth {
        let cfg = cellarr_core::AuthConfig {
            method: auth.method,
            username: auth.username.clone(),
            password_hash: auth.password_hash.clone(),
        };
        if cfg.needs_setup() {
            return Err(ManagedError::Validation(format!(
                "auth selects the enforcing method `{}` but declares no credential \
                 (username + passwordHash); that would lock the operator out — declare \
                 a credential or use method `none`",
                auth.method.as_str()
            )));
        }
    }

    Ok(())
}

/// Build a case-insensitive lowercase name set from an optional section.
fn name_set<T>(items: Option<&[T]>, name_of: impl Fn(&T) -> &String) -> BTreeSet<String> {
    items
        .unwrap_or_default()
        .iter()
        .map(|i| name_of(i).to_ascii_lowercase())
        .collect()
}

/// Whether `set` (already lowercase) contains `name` case-insensitively.
fn contains_ci(set: &BTreeSet<String>, name: &str) -> bool {
    set.contains(&name.to_ascii_lowercase())
}

/// Reject a duplicate (case-insensitive) name within a section.
fn unique_names<'a>(
    kind: &str,
    names: impl Iterator<Item = &'a String>,
) -> Result<(), ManagedError> {
    let mut seen = BTreeSet::new();
    for name in names {
        let key = name.to_ascii_lowercase();
        if !seen.insert(key) {
            return Err(ManagedError::Validation(format!(
                "duplicate {kind} name `{name}` (names must be unique within a section)"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed::loader::load_str;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn valid_cross_refs_pass() {
        let text = r#"
apiVersion: cellarr/v1
rootFolders:
  - name: movies
    path: /data/movies
qualityProfiles:
  - name: HD
    qualities: [Bluray-1080p]
libraries:
  - name: Movies
    mediaType: movie
    rootFolders: [movies]
    qualityProfile: HD
"#;
        load_str(text, no_env).unwrap();
    }

    #[test]
    fn library_missing_profile_is_rejected() {
        let text = r#"
apiVersion: cellarr/v1
rootFolders:
  - name: movies
    path: /data/movies
libraries:
  - name: Movies
    mediaType: movie
    rootFolders: [movies]
    qualityProfile: DoesNotExist
"#;
        let err = load_str(text, no_env).unwrap_err();
        assert!(matches!(err, ManagedError::Validation(_)));
        assert!(err.to_string().contains("DoesNotExist"));
    }

    #[test]
    fn library_missing_root_folder_is_rejected() {
        let text = r#"
apiVersion: cellarr/v1
qualityProfiles:
  - name: HD
    qualities: [Bluray-1080p]
libraries:
  - name: Movies
    mediaType: movie
    rootFolders: [nope]
    qualityProfile: HD
"#;
        let err = load_str(text, no_env).unwrap_err();
        assert!(err.to_string().contains("nope"), "got {err}");
    }

    #[test]
    fn profile_unknown_quality_is_rejected() {
        let text = r#"
apiVersion: cellarr/v1
qualityProfiles:
  - name: HD
    qualities: [NotAQuality]
"#;
        let err = load_str(text, no_env).unwrap_err();
        assert!(err.to_string().contains("NotAQuality"), "got {err}");
    }

    #[test]
    fn profile_scoring_undeclared_custom_format_is_rejected() {
        let text = r#"
apiVersion: cellarr/v1
qualityProfiles:
  - name: HD
    qualities: [Bluray-1080p]
    customFormatScores:
      ghost: 10
"#;
        let err = load_str(text, no_env).unwrap_err();
        assert!(err.to_string().contains("ghost"), "got {err}");
    }

    #[test]
    fn profile_score_must_match_custom_format_own_score() {
        // The CF declares score 50; the profile references it as 10 → rejected (the
        // CF's own score is authoritative).
        let text = r#"
apiVersion: cellarr/v1
customFormats:
  - name: x265
    score: 50
qualityProfiles:
  - name: HD
    qualities: [Bluray-1080p]
    customFormatScores:
      x265: 10
"#;
        let err = load_str(text, no_env).unwrap_err();
        assert!(
            err.to_string().contains("authoritative") || err.to_string().contains("x265"),
            "got {err}"
        );

        // Agreeing scores validate fine.
        let ok = text.replace("x265: 10", "x265: 50");
        load_str(&ok, no_env).expect("matching scores validate");
    }

    #[test]
    fn duplicate_names_rejected() {
        let text = r#"
apiVersion: cellarr/v1
rootFolders:
  - name: dup
    path: /a
  - name: DUP
    path: /b
"#;
        let err = load_str(text, no_env).unwrap_err();
        assert!(err.to_string().contains("duplicate"), "got {err}");
    }

    #[test]
    fn quality_definition_unknown_name_rejected() {
        let text = r#"
apiVersion: cellarr/v1
qualityDefinitions:
  - name: Bluray-9001p
    minSizePerMin: 1
"#;
        let err = load_str(text, no_env).unwrap_err();
        assert!(err.to_string().contains("Bluray-9001p"), "got {err}");
    }

    #[test]
    fn indexer_scoped_to_undeclared_tag_rejected() {
        let text = r#"
apiVersion: cellarr/v1
indexers:
  - name: ix
    kind: torznab
    protocol: torrent
    tags: [missing]
    settings: {}
"#;
        let err = load_str(text, no_env).unwrap_err();
        assert!(err.to_string().contains("missing"), "got {err}");
    }

    #[test]
    fn cutoff_not_in_allowed_rejected() {
        let text = r#"
apiVersion: cellarr/v1
qualityProfiles:
  - name: HD
    qualities: [Bluray-1080p]
    cutoff: WEBDL-720p
"#;
        let err = load_str(text, no_env).unwrap_err();
        assert!(err.to_string().contains("cutoff"), "got {err}");
    }

    #[test]
    fn cardigann_indexer_without_definition_rejected() {
        let text = r#"
apiVersion: cellarr/v1
indexers:
  - name: ct
    kind: cardigann
    protocol: torrent
    settings: {}
"#;
        let err = load_str(text, no_env).unwrap_err();
        assert!(err.to_string().contains("definition"), "got {err}");
    }

    #[test]
    fn cardigann_indexer_with_invalid_definition_rejected() {
        // Valid YAML, but not a valid Cardigann definition (missing `name`/`search`).
        let text = r#"
apiVersion: cellarr/v1
indexers:
  - name: ct
    kind: cardigann
    protocol: torrent
    settings:
      definition: |
        id: only-id
"#;
        let err = load_str(text, no_env).unwrap_err();
        assert!(
            err.to_string().contains("invalid cardigann definition"),
            "got {err}"
        );
    }

    #[test]
    fn cardigann_indexer_with_valid_definition_passes() {
        let text = r#"
apiVersion: cellarr/v1
indexers:
  - name: ct
    kind: cardigann
    protocol: torrent
    settings:
      definition: |
        id: ct
        name: Cardigann Tracker
        links: [https://ct.example/]
        search:
          paths: [{ path: /s }]
          rows: { selector: tr }
          fields:
            title: { selector: a }
            download: { selector: a, attribute: href }
"#;
        load_str(text, no_env).unwrap();
    }

    #[test]
    fn cardigann_indexer_with_definition_file_passes() {
        // A definitionFile is read on the target host at runtime; validation only
        // checks that a source is declared.
        let text = r#"
apiVersion: cellarr/v1
indexers:
  - name: ct
    kind: cardigann
    protocol: torrent
    settings:
      definitionFile: /config/definitions/ct.yml
"#;
        load_str(text, no_env).unwrap();
    }
}
