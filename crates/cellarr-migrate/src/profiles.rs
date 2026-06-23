//! Mapping *arr quality profiles and custom formats onto cellarr's model.
//!
//! The originals and cellarr share the TRaSH-compatible vocabulary, which is the
//! whole reason a user's tuning can come across intact:
//!
//! - **Custom formats**: Sonarr/Radarr store a CF's `Specifications` as JSON with
//!   the same `implementation` / `fields` shape TRaSH publishes, so we route them
//!   straight through [`cellarr_decide::convert`] — the one mapping the decision
//!   engine already trusts. Scores live on the *profile's* `FormatItems`, so we
//!   collect them per CF id and hand them in as the score map.
//! - **Quality profiles**: the originals address qualities by an internal numeric
//!   id; cellarr ranks by its own catalogue. We bridge by **quality name** against
//!   [`cellarr_core::QualityRanking`] (normalizing the few names the apps spell
//!   differently), so a profile that allowed "Bluray-1080p" still allows the same
//!   bucket in cellarr and reproduces equivalent decisions.

use std::collections::HashMap;

use cellarr_core::{CustomFormat, Quality, QualityProfile, QualityProfileId, QualityRanking};
use cellarr_decide::{convert as convert_trash, TrashApp, TrashCustomFormat};
use serde::Deserialize;

use crate::error::{MigrationError, Result};

/// A row from the source `CustomFormats` table.
#[derive(Debug, Clone)]
pub(crate) struct SourceCustomFormat {
    /// The source's internal numeric id (referenced by profile `FormatItems`).
    pub id: i64,
    /// Display name.
    pub name: String,
    /// The raw `Specifications` JSON column.
    pub specifications_json: String,
}

/// A row from the source `QualityProfiles` table.
#[derive(Debug, Clone)]
pub(crate) struct SourceQualityProfile {
    /// Display name.
    pub name: String,
    /// The raw `Items` JSON column (nested quality groups).
    pub items_json: String,
    /// The cutoff quality id.
    pub cutoff: Option<i64>,
    /// Minimum custom-format score (`MinFormatScore`).
    pub min_format_score: i64,
    /// Stop-upgrading custom-format score (`CutoffFormatScore`).
    pub cutoff_format_score: i64,
    /// `FormatItems`: per-CF score assignments, raw JSON.
    pub format_items_json: Option<String>,
    /// Whether upgrades are allowed (`UpgradeAllowed`).
    pub upgrade_allowed: bool,
}

/// One quality item inside a profile's `Items` JSON. The originals nest qualities
/// inside named groups; both a leaf and a group carry an `allowed` flag.
#[derive(Debug, Deserialize)]
struct QualityItem {
    #[serde(default)]
    quality: Option<QualityRef>,
    #[serde(default)]
    items: Vec<QualityItem>,
    #[serde(default)]
    allowed: bool,
}

/// The `{id,name}` quality reference the originals embed.
#[derive(Debug, Deserialize)]
struct QualityRef {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    name: String,
}

/// One `FormatItems` entry: a CF id and the score the profile assigns it.
#[derive(Debug, Deserialize)]
struct FormatItem {
    #[serde(default)]
    format: i64,
    #[serde(default)]
    score: i32,
}

/// Map source custom formats into core [`CustomFormat`]s, scored by `scores`
/// (keyed by source CF id).
///
/// `app` selects the `SourceSpecification` enum dialect: Sonarr and Radarr key
/// the same `value` index to different sources, so migrating a Radarr install
/// with the Sonarr mapping would silently mis-match source-conditioned CFs.
///
/// # Errors
/// Returns [`MigrationError::Json`] for malformed `Specifications` and
/// [`MigrationError::CustomFormat`] for an unrecognized specification kind.
pub(crate) fn map_custom_formats(
    sources: &[SourceCustomFormat],
    scores: &HashMap<i64, i32>,
    app: TrashApp,
) -> Result<Vec<CustomFormat>> {
    let mut out = Vec::with_capacity(sources.len());
    for src in sources {
        let specs: Vec<cellarr_decide::TrashSpecification> =
            serde_json::from_str(&src.specifications_json).map_err(|source| {
                MigrationError::Json {
                    context: format!("custom format {:?} Specifications", src.name),
                    source,
                }
            })?;
        // Reuse the decision engine's own TRaSH converter so imported formats are
        // byte-for-byte the same shape a TRaSH/Recyclarr sync would produce.
        let trash = TrashCustomFormat {
            // No trash_id here: the score is supplied directly via the per-CF map,
            // keyed below by name through `score_map`.
            trash_id: Some(src.name.clone()),
            name: src.name.clone(),
            specifications: specs,
        };
        let mut score_map = HashMap::new();
        if let Some(score) = scores.get(&src.id) {
            score_map.insert(src.name.clone(), *score);
        }
        out.push(convert_trash(trash, &score_map, app)?);
    }
    Ok(out)
}

/// Extract the per-CF score map (source CF id -> score) from every profile's
/// `FormatItems`. When the same CF is scored by multiple profiles the last one
/// wins; cellarr scores a CF once globally, so this is a documented flattening.
pub(crate) fn collect_format_scores(profiles: &[SourceQualityProfile]) -> HashMap<i64, i32> {
    let mut scores = HashMap::new();
    for p in profiles {
        let Some(json) = &p.format_items_json else {
            continue;
        };
        let items: Vec<FormatItem> = serde_json::from_str(json).unwrap_or_default();
        for item in items {
            if item.score != 0 {
                scores.insert(item.format, item.score);
            }
        }
    }
    scores
}

/// Map a source quality profile into a core [`QualityProfile`], resolving
/// allowed qualities and the cutoff against cellarr's [`QualityRanking`].
///
/// # Errors
/// Returns [`MigrationError::Json`] when the `Items` JSON is malformed.
pub(crate) fn map_quality_profile(
    src: &SourceQualityProfile,
    ranking: &QualityRanking,
) -> Result<QualityProfile> {
    let items: Vec<QualityItem> =
        serde_json::from_str(&src.items_json).map_err(|source| MigrationError::Json {
            context: format!("quality profile {:?} Items", src.name),
            source,
        })?;

    let mut allowed_ranks = Vec::new();
    // Map source quality id -> core rank as we walk, so the cutoff (an id) can be
    // resolved to a rank afterwards.
    let mut id_to_rank: HashMap<i64, u32> = HashMap::new();
    collect_allowed(&items, ranking, &mut allowed_ranks, &mut id_to_rank);

    allowed_ranks.sort_unstable();
    allowed_ranks.dedup();

    let cutoff_quality = src
        .cutoff
        .and_then(|id| id_to_rank.get(&id).copied())
        // A cutoff that does not resolve (or is absent) falls back to the highest
        // allowed quality, the safe interpretation: "upgrade up to the best I allow".
        .or_else(|| allowed_ranks.iter().copied().max())
        .unwrap_or(0);

    Ok(QualityProfile {
        id: QualityProfileId::new(),
        name: src.name.clone(),
        allowed_qualities: allowed_ranks,
        upgrades_allowed: src.upgrade_allowed,
        cutoff_quality,
        min_custom_format_score: src.min_format_score as i32,
        upgrade_until_custom_format_score: src.cutoff_format_score as i32,
        required_languages: Vec::new(),
    })
}

/// Walk the nested quality items, recording the core rank of every *allowed*
/// quality and building the source-id -> rank map for cutoff resolution.
fn collect_allowed(
    items: &[QualityItem],
    ranking: &QualityRanking,
    allowed: &mut Vec<u32>,
    id_to_rank: &mut HashMap<i64, u32>,
) {
    for item in items {
        if let Some(q) = &item.quality {
            if let Some(rank) = resolve_rank(&q.name, ranking) {
                id_to_rank.insert(q.id, rank);
                if item.allowed {
                    allowed.push(rank);
                }
            }
        }
        if !item.items.is_empty() {
            // A group is allowed when the group flag is set; its member qualities
            // carry their own flags too, so recurse and let each leaf decide.
            collect_allowed(&item.items, ranking, allowed, id_to_rank);
        }
    }
}

/// The `Quality` envelope an *arr file stores: `{"quality":{"id","name"},…}`.
#[derive(Debug, Deserialize)]
struct FileQualityEnvelope {
    quality: QualityRef,
}

/// Resolve a media file's stored quality JSON to a core [`Quality`].
///
/// Falls back to the catalogue's `Unknown` (rank 0) when the name is one cellarr
/// does not catalogue — the file is still recognized in place, just at unknown
/// quality, which never spuriously triggers an upgrade.
///
/// # Errors
/// Returns [`MigrationError::Json`] when the quality JSON is malformed.
pub(crate) fn map_file_quality(
    quality_json: &str,
    ranking: &QualityRanking,
    context: &str,
) -> Result<Quality> {
    let env: FileQualityEnvelope =
        serde_json::from_str(quality_json).map_err(|source| MigrationError::Json {
            context: context.to_string(),
            source,
        })?;
    Ok(resolve_rank(&env.quality.name, ranking)
        .and_then(|rank| ranking.qualities.iter().find(|q| q.rank == rank))
        .map(|q| Quality::new(q.name.clone(), q.rank))
        .unwrap_or_else(|| Quality::new("Unknown", 0)))
}

/// Resolve a source quality *name* to a cellarr rank, normalizing the spellings
/// the originals use that differ from cellarr's catalogue.
fn resolve_rank(name: &str, ranking: &QualityRanking) -> Option<u32> {
    if let Some(q) = ranking.by_name(name) {
        return Some(q.rank);
    }
    let normalized = normalize_quality_name(name);
    ranking.by_name(&normalized).map(|q| q.rank)
}

/// Normalize the *arr quality names to cellarr's catalogue names.
///
/// The apps use a few different spellings (`WEBDL-1080p` vs cellarr's
/// `WEBDL-1080p` match, but `Remux-1080p` vs `Bluray-1080p Remux`, `WEB 480p`
/// group names, etc.). This encodes the non-obvious naming facts; unknown names
/// pass through unchanged and simply fail to resolve (the quality is dropped from
/// the profile rather than silently mismapped).
fn normalize_quality_name(name: &str) -> String {
    match name {
        // Radarr/Sonarr remux spellings -> cellarr's "<base> Remux".
        "Remux-1080p" | "Bluray-1080p-Remux" => "Bluray-1080p Remux".to_string(),
        "Remux-2160p" | "Bluray-2160p-Remux" => "Bluray-2160p Remux".to_string(),
        // WEB-DL spelled with a hyphen in some exports.
        other if other.starts_with("WEB-DL") => other.replacen("WEB-DL", "WEBDL", 1),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(items: &str, cutoff: Option<i64>) -> SourceQualityProfile {
        SourceQualityProfile {
            name: "P".to_string(),
            items_json: items.to_string(),
            cutoff,
            min_format_score: 0,
            cutoff_format_score: 100,
            format_items_json: None,
            upgrade_allowed: true,
        }
    }

    #[test]
    fn maps_allowed_qualities_and_cutoff_by_name() {
        let ranking = QualityRanking::default();
        let src = profile(
            r#"[{"quality":{"id":1,"name":"SDTV"},"allowed":false},
                {"quality":{"id":3,"name":"WEBDL-1080p"},"allowed":true},
                {"quality":{"id":7,"name":"Bluray-1080p"},"allowed":true}]"#,
            Some(7),
        );
        let mapped = map_quality_profile(&src, &ranking).unwrap();

        let webdl = ranking.by_name("WEBDL-1080p").unwrap().rank;
        let bluray = ranking.by_name("Bluray-1080p").unwrap().rank;
        let sdtv = ranking.by_name("SDTV").unwrap().rank;

        assert!(mapped.allowed_qualities.contains(&webdl));
        assert!(mapped.allowed_qualities.contains(&bluray));
        assert!(
            !mapped.allowed_qualities.contains(&sdtv),
            "disallowed quality is excluded"
        );
        // Cutoff id 7 resolves to the Bluray-1080p rank.
        assert_eq!(mapped.cutoff_quality, bluray);
        assert!(mapped.upgrades_allowed);
    }

    #[test]
    fn nested_quality_groups_are_flattened() {
        let ranking = QualityRanking::default();
        let src = profile(
            r#"[{"name":"WEB 1080p","allowed":true,"items":[
                  {"quality":{"id":3,"name":"WEBDL-1080p"},"allowed":true},
                  {"quality":{"id":2,"name":"WEBRip-1080p"},"allowed":true}]}]"#,
            None,
        );
        let mapped = map_quality_profile(&src, &ranking).unwrap();
        assert!(mapped
            .allowed_qualities
            .contains(&ranking.by_name("WEBDL-1080p").unwrap().rank));
        assert!(mapped
            .allowed_qualities
            .contains(&ranking.by_name("WEBRip-1080p").unwrap().rank));
    }

    #[test]
    fn remux_name_normalizes_to_cellarr_catalogue() {
        let ranking = QualityRanking::default();
        assert_eq!(
            resolve_rank("Remux-2160p", &ranking),
            ranking.by_name("Bluray-2160p Remux").map(|q| q.rank)
        );
    }

    #[test]
    fn custom_format_specs_route_through_decide() {
        let scores = HashMap::from([(1_i64, 75_i32)]);
        let sources = vec![SourceCustomFormat {
            id: 1,
            name: "HDR10".to_string(),
            specifications_json: r#"[{"name":"HDR10","implementation":"ReleaseTitleSpecification","required":true,"negate":false,"fields":{"value":"\\bHDR10\\b"}}]"#.to_string(),
        }];
        let cfs = map_custom_formats(&sources, &scores, TrashApp::Sonarr).unwrap();
        assert_eq!(cfs.len(), 1);
        assert_eq!(cfs[0].name, "HDR10");
        assert_eq!(
            cfs[0].score, 75,
            "score comes from the profile FormatItems map"
        );
        assert_eq!(cfs[0].conditions.len(), 1);
    }

    #[test]
    fn file_quality_resolves_against_catalogue() {
        let ranking = QualityRanking::default();
        let q = map_file_quality(
            r#"{"quality":{"id":7,"name":"Bluray-1080p"}}"#,
            &ranking,
            "test",
        )
        .unwrap();
        assert_eq!(q.name, "Bluray-1080p");
        assert_eq!(q.rank, ranking.by_name("Bluray-1080p").unwrap().rank);
    }

    #[test]
    fn unknown_file_quality_falls_back_to_unknown() {
        let ranking = QualityRanking::default();
        let q = map_file_quality(
            r#"{"quality":{"id":999,"name":"Totally-Made-Up"}}"#,
            &ranking,
            "test",
        )
        .unwrap();
        assert_eq!(q.name, "Unknown");
        assert_eq!(q.rank, 0);
    }
}
