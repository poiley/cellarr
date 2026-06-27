//! Export the current DB state of the managed-able kinds as a [`ManagedConfig`].
//!
//! This is the inverse of reconciliation: it reads every managed-able kind out of
//! the live database and renders a [`ManagedConfig`] that, fed back through
//! `config validate`/reconcile, produces an empty plan (round-trippable). It is
//! how an operator captures a hand-configured instance into config-as-code, or
//! reviews drift.
//!
//! **Secrets are never emitted.** A string value in an indexer's or download
//! client's `settings` whose key looks secret (`apiKey`, `password`, `passkey`,
//! …) is replaced with a `${ENV}` placeholder derived from the entity + field, so
//! the exported file is safe to commit and the operator wires the real secret into
//! the environment. This is documented behavior, not a silent redaction.

use cellarr_core::repo::ProfileRepository;
use cellarr_core::QualityRanking;
use cellarr_db::Database;

use crate::managed::error::ManagedError;
use crate::managed::schema::{
    CustomFormatSpec, DownloadClientSpec, IndexerSpec, LibrarySpec, ManagedConfig,
    QualityDefinitionSpec, QualityProfileSpec, RootFolderSpec, TagSpec, SUPPORTED_API_VERSION,
};

/// The substring markers (case-insensitive) that identify a settings key as
/// secret-bearing, so its value is emitted as a `${ENV}` placeholder.
const SECRET_KEY_MARKERS: &[&str] = &["apikey", "password", "passkey", "secret", "token"];

/// Read the whole managed surface out of `db` and render it as a [`ManagedConfig`].
///
/// Every section is emitted (as `Some`), so the export is a complete snapshot the
/// operator can prune. Secret-bearing settings are emitted as `${ENV}` placeholders.
///
/// # Errors
/// Returns a [`ManagedError`] if reading the DB fails.
pub async fn export(db: &Database) -> Result<ManagedConfig, ManagedError> {
    let ranking = db.profiles().quality_ranking().await?;

    let tags = db
        .tags()
        .list()
        .await?
        .into_iter()
        .map(|t| TagSpec { name: t.label })
        .collect();

    // Only emit quality-definition rows that were actually edited (the override
    // set), not the entire code-owned catalogue.
    let quality_definitions = db
        .profiles()
        .quality_definition_overrides()
        .await?
        .into_iter()
        .map(|d| QualityDefinitionSpec {
            name: d.name,
            title: d.title,
            min_size_per_min: d.min_size_per_min,
            max_size_per_min: d.max_size_per_min,
            preferred_size_per_min: d.preferred_size_per_min,
        })
        .collect();

    let custom_formats: Vec<CustomFormatSpec> = db
        .profiles()
        .custom_formats()
        .await?
        .into_iter()
        .map(|c| CustomFormatSpec {
            name: c.name,
            score: c.score,
            conditions: c.conditions,
        })
        .collect();

    let quality_profiles = export_profiles(db, &ranking, &custom_formats).await?;
    let root_folders = export_root_folders(db).await?;
    let libraries = export_libraries(db).await?;
    let indexers = export_indexers(db).await?;
    let download_clients = export_download_clients(db).await?;

    Ok(ManagedConfig {
        api_version: SUPPORTED_API_VERSION.to_string(),
        version: None,
        tags: Some(tags),
        quality_definitions: Some(quality_definitions),
        custom_formats: Some(custom_formats),
        quality_profiles: Some(quality_profiles),
        root_folders: Some(root_folders),
        libraries: Some(libraries),
        indexers: Some(indexers),
        download_clients: Some(download_clients),
    })
}

/// Serialize an exported config to a YAML string.
///
/// # Errors
/// Returns a [`ManagedError::Parse`] if serialization fails (it should not for a
/// well-formed config).
pub fn to_yaml(config: &ManagedConfig) -> Result<String, ManagedError> {
    serde_yaml::to_string(config).map_err(|e| ManagedError::Parse(e.to_string()))
}

async fn export_profiles(
    db: &Database,
    ranking: &QualityRanking,
    custom_formats: &[CustomFormatSpec],
) -> Result<Vec<QualityProfileSpec>, ManagedError> {
    let profiles = db.profiles().list_profiles().await?;
    // Rank → name lookup for rendering allowed qualities + cutoff by name.
    let name_for_rank = |rank: u32| -> Option<String> {
        ranking
            .qualities
            .iter()
            .find(|q| q.rank == rank)
            .map(|q| q.name.clone())
    };
    Ok(profiles
        .into_iter()
        .map(|p| {
            let qualities = p
                .allowed_qualities
                .iter()
                .filter_map(|r| name_for_rank(*r))
                .collect();
            let cutoff = name_for_rank(p.cutoff_quality);
            // Emit the custom-format scores this profile differs from CF default on
            // — i.e. every CF whose score is non-zero, keyed by name (round-trips to
            // the same CF score on re-import since the CF carries the score too).
            let custom_format_scores = custom_formats
                .iter()
                .filter(|cf| cf.score != 0)
                .map(|cf| (cf.name.clone(), cf.score))
                .collect();
            QualityProfileSpec {
                name: p.name,
                qualities,
                upgrades_allowed: p.upgrades_allowed,
                cutoff,
                min_custom_format_score: p.min_custom_format_score,
                upgrade_until_custom_format_score: p.upgrade_until_custom_format_score,
                required_languages: p.required_languages,
                custom_format_scores,
            }
        })
        .collect())
}

async fn export_root_folders(db: &Database) -> Result<Vec<RootFolderSpec>, ManagedError> {
    Ok(db
        .config()
        .list_root_folders()
        .await?
        .into_iter()
        .map(|rf| RootFolderSpec {
            // Prefer the human label; fall back to the id so the export is never
            // nameless (the name is the reconcile identity).
            name: rf.name.clone().unwrap_or_else(|| rf.id.clone()),
            path: rf.path,
            enabled: rf.enabled,
        })
        .collect())
}

async fn export_libraries(db: &Database) -> Result<Vec<LibrarySpec>, ManagedError> {
    let libraries = db.config().list_libraries().await?;
    let profiles = db.profiles().list_profiles().await?;
    let root_folders = db.config().list_root_folders().await?;
    Ok(libraries
        .into_iter()
        .map(|lib| {
            let quality_profile = profiles
                .iter()
                .find(|p| p.id == lib.default_quality_profile)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| lib.default_quality_profile.to_string());
            // Map stored root-folder ids back to their names so the export
            // references by name (the schema's contract).
            let root_folder_names = lib
                .root_folders
                .iter()
                .map(|id| {
                    root_folders
                        .iter()
                        .find(|rf| &rf.id == id)
                        .and_then(|rf| rf.name.clone())
                        .unwrap_or_else(|| id.clone())
                })
                .collect();
            LibrarySpec {
                name: lib.name,
                media_type: lib.media_type,
                root_folders: root_folder_names,
                quality_profile,
            }
        })
        .collect())
}

async fn export_indexers(db: &Database) -> Result<Vec<IndexerSpec>, ManagedError> {
    let indexers = db.config().list_indexers().await?;
    let tags = db.tags().list().await?;
    Ok(indexers
        .into_iter()
        .map(|ix| {
            let tag_names = tag_names_for(&ix.tags, &tags);
            let settings = redact_secrets(&ix.name, ix.settings);
            IndexerSpec {
                name: ix.name,
                kind: ix.kind,
                protocol: ix.protocol,
                enabled: ix.enabled,
                priority: ix.priority,
                criteria: ix.criteria,
                tags: tag_names,
                settings,
            }
        })
        .collect())
}

async fn export_download_clients(db: &Database) -> Result<Vec<DownloadClientSpec>, ManagedError> {
    let clients = db.config().list_download_clients().await?;
    let tags = db.tags().list().await?;
    Ok(clients
        .into_iter()
        .map(|dc| {
            let tag_names = tag_names_for(&dc.tags, &tags);
            let settings = redact_secrets(&dc.name, dc.settings);
            DownloadClientSpec {
                name: dc.name,
                kind: dc.kind,
                protocol: dc.protocol,
                enabled: dc.enabled,
                priority: dc.priority,
                category: dc.category,
                tags: tag_names,
                settings,
            }
        })
        .collect())
}

/// Map a tag-id list back to labels, dropping ids that no longer exist.
fn tag_names_for(ids: &[u32], tags: &[cellarr_core::Tag]) -> Vec<String> {
    ids.iter()
        .filter_map(|id| tags.iter().find(|t| t.id == *id).map(|t| t.label.clone()))
        .collect()
}

/// Replace secret-bearing string values in a `settings` object with a `${ENV}`
/// placeholder so the export never leaks a key/password. The placeholder name is
/// derived from the entity name + field so it is unique and self-documenting.
fn redact_secrets(entity_name: &str, settings: serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(map) = settings else {
        return settings;
    };
    let env_prefix = sanitize_env(entity_name);
    let redacted = map
        .into_iter()
        .map(|(key, value)| {
            let is_secret = SECRET_KEY_MARKERS
                .iter()
                .any(|m| key.to_ascii_lowercase().contains(m));
            if is_secret && value.is_string() {
                let placeholder = format!("${{{env_prefix}_{}}}", sanitize_env(&key));
                (key, serde_json::Value::String(placeholder))
            } else {
                (key, value)
            }
        })
        .collect();
    serde_json::Value::Object(redacted)
}

/// Render a name into an uppercase, underscore-only token safe for an env-var name.
fn sanitize_env(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secret_keys_to_env_placeholders() {
        let settings = serde_json::json!({
            "baseUrl": "https://x",
            "apiKey": "super-secret",
            "categories": [1, 2],
        });
        let out = redact_secrets("nzbgeek", settings);
        assert_eq!(out["baseUrl"], "https://x");
        assert_eq!(out["apiKey"], "${NZBGEEK_APIKEY}");
        // Non-string / non-secret values pass through untouched.
        assert_eq!(out["categories"], serde_json::json!([1, 2]));
    }

    #[test]
    fn sanitize_env_uppercases_and_replaces() {
        assert_eq!(sanitize_env("nzb-geek 1"), "NZB_GEEK_1");
    }
}
