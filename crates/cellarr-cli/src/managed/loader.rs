//! Loading a [`ManagedConfig`] from a file: read → interpolate → parse → validate.
//!
//! This is the single entry point that turns a path on disk into a validated,
//! secret-resolved [`ManagedConfig`]. The order matters:
//!
//! 1. **Read** the raw file text.
//! 2. **Interpolate** `${ENV}` references against the process environment, so a
//!    secret is resolved before the YAML is even parsed (and a missing required
//!    secret fails here, naming the variable).
//! 3. **Parse** the (now secret-resolved) text into the typed schema; an unknown
//!    field or type mismatch is a hard error (`deny_unknown_fields`).
//! 4. **Check the apiVersion** matches this build.
//! 5. **Validate** cross-references and uniqueness (delegated to [`super::validate`]).
//!
//! A failure at any step is a typed [`ManagedError`] with a clear message.

use std::path::Path;

use crate::managed::error::ManagedError;
use crate::managed::interpolate::interpolate;
use crate::managed::schema::{ManagedConfig, SUPPORTED_API_VERSION};
use crate::managed::validate;

/// Load, interpolate, parse, and validate a managed config from `path`, resolving
/// `${ENV}` references against the real process environment.
///
/// # Errors
/// Returns a [`ManagedError`] for an unreadable file, a malformed/unknown-field
/// YAML, an unresolved required secret, an unsupported apiVersion, or a failed
/// cross-reference / uniqueness check.
pub fn load(path: &Path) -> Result<ManagedConfig, ManagedError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ManagedError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    load_str(&raw, |k| std::env::var(k).ok())
}

/// The pure core of [`load`]: interpolate `text` with `lookup`, parse, version-
/// check, and validate. Separated so tests drive it with an explicit environment
/// and an in-memory string (no real file, no real env).
///
/// # Errors
/// As [`load`].
pub fn load_str<F>(text: &str, lookup: F) -> Result<ManagedConfig, ManagedError>
where
    F: FnMut(&str) -> Option<String>,
{
    // 1. Resolve secrets on the raw text (missing required => error here).
    let resolved = interpolate(text, lookup)?;

    // 2. Parse into the strict schema.
    let config: ManagedConfig =
        serde_yaml::from_str(&resolved).map_err(|e| ManagedError::Parse(e.to_string()))?;

    // 3. Version gate.
    if config.api_version != SUPPORTED_API_VERSION {
        return Err(ManagedError::UnsupportedApiVersion {
            found: config.api_version.clone(),
            supported: SUPPORTED_API_VERSION,
        });
    }

    // 4. Semantic validation (cross-refs, uniqueness).
    validate::validate(&config)?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn loads_minimal_valid_config() {
        let cfg = load_str("apiVersion: cellarr/v1\n", no_env).unwrap();
        assert_eq!(cfg.api_version, SUPPORTED_API_VERSION);
    }

    #[test]
    fn malformed_yaml_is_a_parse_error() {
        let err = load_str("apiVersion: cellarr/v1\n  : : bad", no_env).unwrap_err();
        assert!(matches!(err, ManagedError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn unknown_field_is_a_parse_error() {
        let err = load_str("apiVersion: cellarr/v1\nbogusSection: []\n", no_env).unwrap_err();
        assert!(matches!(err, ManagedError::Parse(_)), "got {err:?}");
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn wrong_api_version_is_rejected() {
        let err = load_str("apiVersion: cellarr/v2\n", no_env).unwrap_err();
        assert!(
            matches!(err, ManagedError::UnsupportedApiVersion { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn missing_required_secret_errors_before_parse() {
        let text = "apiVersion: cellarr/v1\nindexers:\n  - name: ix\n    kind: torznab\n    protocol: torrent\n    settings:\n      apiKey: ${MISSING_KEY}\n";
        let err = load_str(text, no_env).unwrap_err();
        match err {
            ManagedError::UnresolvedSecret { var } => assert_eq!(var, "MISSING_KEY"),
            other => panic!("expected UnresolvedSecret, got {other:?}"),
        }
    }

    #[test]
    fn provided_secret_is_resolved_into_settings() {
        let text = "apiVersion: cellarr/v1\nindexers:\n  - name: ix\n    kind: torznab\n    protocol: torrent\n    settings:\n      apiKey: ${IX_KEY}\n";
        let cfg = load_str(text, |k| {
            (k == "IX_KEY").then(|| "resolved-secret".to_string())
        })
        .unwrap();
        let ix = &cfg.indexers.unwrap()[0];
        assert_eq!(ix.settings["apiKey"], "resolved-secret");
    }

    #[test]
    fn loads_from_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("managed.yaml");
        std::fs::write(&path, "apiVersion: cellarr/v1\nrootFolders: []\n").unwrap();
        let cfg = load(&path).unwrap();
        assert_eq!(cfg.root_folders, Some(Vec::new()));
    }

    #[test]
    fn missing_file_is_a_read_error() {
        let err = load(Path::new("/nonexistent/managed.yaml")).unwrap_err();
        assert!(matches!(err, ManagedError::Read { .. }), "got {err:?}");
    }
}
