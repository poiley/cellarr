//! Importing TRaSH-Guides / Recyclarr custom-format JSON into [`CustomFormat`].
//!
//! TRaSH publishes custom formats as JSON keyed by a `trash_id`, each carrying a
//! list of `specifications` (conditions). Recommended **scores** live separately
//! (in the guide's quality-profile JSON), so an import takes the CF definitions
//! *and* a scores map (trash_id -> score). A format absent from the map imports
//! with score 0 (unscored but still matchable), matching how Recyclarr treats
//! CFs you sync but do not assign a score.
//!
//! We map TRaSH `implementation` discriminators onto core's [`ConditionKind`].
//! The mapping is intentionally a *superset-compatible* subset: the kinds core
//! models import losslessly; an unrecognized implementation is a hard error so a
//! silent miss can never quietly change a user's decisions.

use std::collections::HashMap;

use cellarr_core::{
    Condition, ConditionKind, CustomFormat, CustomFormatId, HdrFormat, ProperRepack, Resolution,
    Source, VideoCodec,
};
use serde::Deserialize;

use crate::error::DecideError;
use crate::matching::format_regexes_compile;

/// The synthetic skip label used when a custom format imported cleanly (all spec
/// kinds modelled) but one of its title regexes uses a .NET-only construct
/// cellarr's regex engine cannot compile (variable-size look-behind, `[` inside
/// a character class). Reported as a skip so it is counted, never silently
/// matched-as-false.
pub const INCOMPATIBLE_REGEX_LABEL: &str = "ReleaseTitleSpecification(incompatible-regex)";

/// Which app's enum dialect a TRaSH CF set is written against.
///
/// Sonarr and Radarr publish CFs keyed to **different** `QualitySource` enum
/// indices (verified live against both apps' `/api/v3/parse`):
///
/// * Sonarr: television=1, web=3, webRip=4, dvd=5, bluray=6, blurayRaw=7
/// * Radarr: cam=1, telesync=2, telecine=3, workprint=4, dvd=5, tv=6, webdl=7,
///   webrip=8, bluray=9
///
/// A `SourceSpecification` index therefore means different things in the two
/// sets; importing a Radarr CF with the Sonarr mapping (or vice-versa) silently
/// changes which releases it matches. The importer takes the dialect explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrashApp {
    /// Sonarr's `QualitySource` enum.
    #[default]
    Sonarr,
    /// Radarr's `QualitySource` enum.
    Radarr,
}

/// A single TRaSH custom format as published in the guide JSON.
#[derive(Debug, Clone, Deserialize)]
pub struct TrashCustomFormat {
    /// The stable guide identifier, used to look up the recommended score.
    #[serde(default)]
    pub trash_id: Option<String>,
    /// Human-facing name.
    pub name: String,
    /// The condition list.
    #[serde(default)]
    pub specifications: Vec<TrashSpecification>,
}

/// One specification (condition) within a TRaSH custom format.
#[derive(Debug, Clone, Deserialize)]
pub struct TrashSpecification {
    /// The implementation discriminator (e.g. "ReleaseTitleSpecification").
    pub implementation: String,
    /// Whether the spec is required (AND).
    #[serde(default)]
    pub required: bool,
    /// Whether the spec is negated (matches on absence).
    #[serde(default)]
    pub negate: bool,
    /// The spec's typed fields.
    #[serde(default)]
    pub fields: TrashFields,
}

/// The `fields` object of a specification. TRaSH uses `value` for most specs and
/// `min`/`max` for size specs.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TrashFields {
    /// A scalar value (regex string, group name, or an enum *index*).
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    /// Minimum, for size specifications (bytes).
    #[serde(default)]
    pub min: Option<u64>,
    /// Maximum, for size specifications (bytes).
    #[serde(default)]
    pub max: Option<u64>,
}

/// Import TRaSH custom formats, assigning each the score found in `scores`
/// (keyed by `trash_id`); formats with no entry get score 0.
///
/// # Errors
/// Returns [`DecideError::TrashJson`] on malformed JSON and
/// [`DecideError::UnsupportedTrashSpec`] for an unrecognized `implementation`.
pub fn import_trash_custom_formats(
    json: &str,
    scores: &HashMap<String, i32>,
) -> Result<Vec<CustomFormat>, DecideError> {
    import_trash_custom_formats_for_app(json, scores, TrashApp::Sonarr)
}

/// Like [`import_trash_custom_formats`] but with an explicit [`TrashApp`] dialect
/// so `SourceSpecification` indices resolve correctly (Sonarr and Radarr differ).
///
/// # Errors
/// Returns [`DecideError::TrashJson`] on malformed JSON and
/// [`DecideError::UnsupportedTrashSpec`] for an unrecognized `implementation`.
pub fn import_trash_custom_formats_for_app(
    json: &str,
    scores: &HashMap<String, i32>,
    app: TrashApp,
) -> Result<Vec<CustomFormat>, DecideError> {
    let raw: Vec<TrashCustomFormat> = serde_json::from_str(json)?;
    raw.into_iter().map(|cf| convert(cf, scores, app)).collect()
}

/// The outcome of a *tolerant* TRaSH import ([`import_trash_custom_formats_counted`]).
///
/// Unlike the strict [`import_trash_custom_formats`], which hard-errors on the
/// first unsupported `implementation`, this reports what was imported and what
/// was skipped — never silently dropping anything. A custom format is skipped
/// whole when *any* of its specifications uses an `implementation` cellarr does
/// not model (importing it partially would change its match semantics), and the
/// skipped implementation discriminators are tallied so callers can ratchet a
/// supported-rate.
#[derive(Debug, Clone, Default)]
pub struct TrashImportReport {
    /// The custom formats that imported (every spec mapped to a [`ConditionKind`]).
    pub formats: Vec<CustomFormat>,
    /// Total custom formats seen in the JSON (supported + skipped).
    pub total: usize,
    /// Names of the custom formats that were skipped, each paired with the
    /// unsupported `implementation` that caused the skip.
    pub skipped: Vec<(String, String)>,
    /// Count of skips per unsupported `implementation` discriminator.
    pub unsupported_counts: HashMap<String, usize>,
}

impl TrashImportReport {
    /// The number of custom formats that imported successfully.
    #[must_use]
    pub fn supported(&self) -> usize {
        self.formats.len()
    }

    /// The fraction of custom formats that imported (1.0 when `total` is 0).
    #[must_use]
    pub fn supported_rate(&self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            self.formats.len() as f64 / self.total as f64
        }
    }
}

/// Import TRaSH custom formats *tolerantly*: every CF whose specifications all
/// map to a [`ConditionKind`] is imported; any CF containing an unsupported
/// `implementation` is **skipped and counted** (never silently dropped), so the
/// whole real-world guide set can be imported and a supported-rate measured.
///
/// Scores are assigned as in [`import_trash_custom_formats`] (by `trash_id`;
/// missing → 0).
///
/// # Errors
/// Returns [`DecideError::TrashJson`] only on malformed JSON. Unsupported specs
/// are reported in the returned [`TrashImportReport`], not raised as errors.
pub fn import_trash_custom_formats_counted(
    json: &str,
    scores: &HashMap<String, i32>,
) -> Result<TrashImportReport, DecideError> {
    import_trash_custom_formats_counted_for_app(json, scores, TrashApp::Sonarr)
}

/// Like [`import_trash_custom_formats_counted`] but with an explicit [`TrashApp`]
/// dialect so `SourceSpecification` indices resolve correctly.
///
/// # Errors
/// Returns [`DecideError::TrashJson`] only on malformed JSON. Unsupported specs
/// are reported in the returned [`TrashImportReport`], not raised as errors.
pub fn import_trash_custom_formats_counted_for_app(
    json: &str,
    scores: &HashMap<String, i32>,
    app: TrashApp,
) -> Result<TrashImportReport, DecideError> {
    let raw: Vec<TrashCustomFormat> = serde_json::from_str(json)?;
    let mut report = TrashImportReport {
        total: raw.len(),
        ..Default::default()
    };
    for cf in raw {
        let name = cf.name.clone();
        match convert(cf, scores, app) {
            Ok(format) if format_regexes_compile(&format) => report.formats.push(format),
            // Imported cleanly but a title regex uses a .NET-only construct
            // cellarr's engine rejects: skip-and-count so it can never be
            // silently treated as a non-match in a real decision.
            Ok(_) => {
                let label = INCOMPATIBLE_REGEX_LABEL.to_string();
                *report.unsupported_counts.entry(label.clone()).or_default() += 1;
                report.skipped.push((name, label));
            }
            Err(DecideError::UnsupportedTrashSpec { implementation, .. }) => {
                *report
                    .unsupported_counts
                    .entry(implementation.clone())
                    .or_default() += 1;
                report.skipped.push((name, implementation));
            }
            // convert only ever fails with UnsupportedTrashSpec; surface anything
            // else (it would be a new, unexpected failure mode) rather than hide it.
            Err(other) => return Err(other),
        }
    }
    Ok(report)
}

/// Convert a single parsed [`TrashCustomFormat`] into a core [`CustomFormat`].
///
/// # Errors
/// Returns [`DecideError::UnsupportedTrashSpec`] for an unrecognized spec.
pub fn convert(
    cf: TrashCustomFormat,
    scores: &HashMap<String, i32>,
    app: TrashApp,
) -> Result<CustomFormat, DecideError> {
    let score = cf
        .trash_id
        .as_ref()
        .and_then(|id| scores.get(id).copied())
        .unwrap_or(0);

    let mut conditions = Vec::with_capacity(cf.specifications.len());
    for spec in &cf.specifications {
        conditions.push(convert_spec(&cf.name, spec, app)?);
    }

    Ok(CustomFormat {
        id: CustomFormatId::new(),
        name: cf.name,
        conditions,
        score,
    })
}

fn convert_spec(
    format_name: &str,
    spec: &TrashSpecification,
    app: TrashApp,
) -> Result<Condition, DecideError> {
    let kind = match spec.implementation.as_str() {
        "ReleaseTitleSpecification" | "ReleaseTitle" => ConditionKind::ReleaseTitle {
            pattern: value_str(spec),
        },
        "ReleaseGroupSpecification" | "ReleaseGroup" => ConditionKind::ReleaseGroup {
            // The apps treat this value as a regex against the parsed group;
            // cellarr-decide::matching compiles and evaluates it as one.
            name: value_str(spec),
        },
        "SourceSpecification" | "Source" => ConditionKind::Source {
            source: source_from_index(spec, app),
        },
        "ResolutionSpecification" | "Resolution" => ConditionKind::Resolution {
            resolution: resolution_from_index(spec),
        },
        "LanguageSpecification" | "Language" => ConditionKind::Language {
            language: value_str(spec),
        },
        "SizeSpecification" | "Size" => ConditionKind::Size {
            min: spec.fields.min,
            max: spec.fields.max,
        },
        "IndexerFlagSpecification" | "IndexerFlag" => ConditionKind::IndexerFlag {
            flag: value_str(spec),
        },
        "QualityModifierSpecification" | "QualityModifier" => ConditionKind::QualityModifier {
            modifier: modifier_from_value(spec),
        },
        other => {
            return Err(DecideError::UnsupportedTrashSpec {
                format: format_name.to_string(),
                implementation: other.to_string(),
            });
        }
    };
    Ok(Condition {
        kind,
        required: spec.required,
        negate: spec.negate,
    })
}

/// The `value` field as a string (regex, group name, language, flag). A numeric
/// value is stringified so e.g. an integer language id still round-trips.
fn value_str(spec: &TrashSpecification) -> String {
    match &spec.fields.value {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// TRaSH encodes Source as the app's `QualitySource` enum index. The two apps use
/// **different** indices (verified live against both `/api/v3/parse`), so the
/// mapping is dialect-specific. An unknown index falls back to the lowest source
/// so it never over-matches.
fn source_from_index(spec: &TrashSpecification, app: TrashApp) -> Source {
    let idx = value_i64(spec);
    match app {
        // Sonarr QualitySource: television=1, televisionRaw=2, web=3, webRip=4,
        // dvd=5, bluray=6, blurayRaw=7. cellarr's Hdtv covers television(Raw);
        // blurayRaw is a full untouched disc (Raw-HD-ish) but the parse axis
        // closest to Sonarr's blurayRaw is Bluray, so map 7 -> Bluray.
        TrashApp::Sonarr => match idx {
            Some(1) => Source::Hdtv,
            Some(2) => Source::Hdtv,
            Some(3) => Source::WebDl,
            Some(4) => Source::Webrip,
            Some(5) => Source::Dvd,
            Some(6) => Source::Bluray,
            Some(7) => Source::Bluray,
            _ => Source::Cam,
        },
        // Radarr QualitySource: cam=1, telesync=2, telecine=3, workprint=4,
        // dvd=5, tv=6, webdl=7, webrip=8, bluray=9. Radarr folds remux into
        // bluray (no distinct source index), so 9 -> Bluray.
        TrashApp::Radarr => match idx {
            Some(1) => Source::Cam,
            Some(2) => Source::Telesync,
            Some(3) => Source::Telecine,
            Some(4) => Source::Workprint,
            Some(5) => Source::Dvd,
            Some(6) => Source::Hdtv,
            Some(7) => Source::WebDl,
            Some(8) => Source::Webrip,
            Some(9) => Source::Bluray,
            _ => Source::Cam,
        },
    }
}

/// TRaSH encodes Resolution as an enum index keyed to pixel height.
fn resolution_from_index(spec: &TrashSpecification) -> Resolution {
    match value_i64(spec) {
        Some(480) => Resolution::R480p,
        Some(576) => Resolution::R576p,
        Some(720) => Resolution::R720p,
        Some(1080) => Resolution::R1080p,
        Some(2160) => Resolution::R2160p,
        _ => Resolution::R480p,
    }
}

fn modifier_from_value(spec: &TrashSpecification) -> ProperRepack {
    match value_str(spec).to_ascii_lowercase().as_str() {
        "repack" => ProperRepack::Repack,
        _ => ProperRepack::Proper,
    }
}

fn value_i64(spec: &TrashSpecification) -> Option<i64> {
    match &spec.fields.value {
        Some(serde_json::Value::Number(n)) => n.as_i64(),
        Some(serde_json::Value::String(s)) => s.parse().ok(),
        _ => None,
    }
}

/// Codec mapping kept for completeness where a guide encodes a codec spec by name.
#[allow(dead_code)]
fn codec_from_value(spec: &TrashSpecification) -> VideoCodec {
    match value_str(spec).to_ascii_lowercase().as_str() {
        "x264" | "h264" | "avc" => VideoCodec::X264,
        "x265" | "h265" | "hevc" => VideoCodec::X265,
        "av1" => VideoCodec::Av1,
        _ => VideoCodec::Other,
    }
}

/// HDR mapping kept for completeness where a guide encodes an HDR spec by name.
#[allow(dead_code)]
fn hdr_from_value(spec: &TrashSpecification) -> HdrFormat {
    match value_str(spec).to_ascii_lowercase().as_str() {
        "hdr10plus" | "hdr10+" => HdrFormat::Hdr10Plus,
        "dolbyvision" | "dv" => HdrFormat::DolbyVision,
        "hlg" => HdrFormat::Hlg,
        _ => HdrFormat::Hdr10,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_spec(idx: i64) -> TrashSpecification {
        TrashSpecification {
            implementation: "SourceSpecification".to_string(),
            required: false,
            negate: false,
            fields: TrashFields {
                value: Some(serde_json::json!(idx)),
                min: None,
                max: None,
            },
        }
    }

    #[test]
    fn source_index_is_app_specific() {
        // Verified live against both apps' /api/v3/parse:
        //   Sonarr: web=3, webRip=4, dvd=5, bluray=6, blurayRaw=7
        //   Radarr: dvd=5, tv=6, webdl=7, webrip=8, bluray=9
        // The SAME index 7 means blurayRaw on Sonarr but WEB-DL on Radarr — so a
        // shared mapping would silently mis-match a whole class of real CFs.
        assert_eq!(
            source_from_index(&source_spec(3), TrashApp::Sonarr),
            Source::WebDl
        );
        assert_eq!(
            source_from_index(&source_spec(4), TrashApp::Sonarr),
            Source::Webrip
        );
        assert_eq!(
            source_from_index(&source_spec(6), TrashApp::Sonarr),
            Source::Bluray
        );

        assert_eq!(
            source_from_index(&source_spec(7), TrashApp::Radarr),
            Source::WebDl
        );
        assert_eq!(
            source_from_index(&source_spec(8), TrashApp::Radarr),
            Source::Webrip
        );
        assert_eq!(
            source_from_index(&source_spec(9), TrashApp::Radarr),
            Source::Bluray
        );

        // Index 7 diverges between the two dialects.
        assert_ne!(
            source_from_index(&source_spec(7), TrashApp::Sonarr),
            source_from_index(&source_spec(7), TrashApp::Radarr),
            "index 7 must mean different sources on Sonarr vs Radarr"
        );
    }
}
