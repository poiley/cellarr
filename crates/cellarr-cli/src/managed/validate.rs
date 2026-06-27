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
}
