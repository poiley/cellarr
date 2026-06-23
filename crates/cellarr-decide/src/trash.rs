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
    let raw: Vec<TrashCustomFormat> = serde_json::from_str(json)?;
    raw.into_iter().map(|cf| convert(cf, scores)).collect()
}

/// Convert a single parsed [`TrashCustomFormat`] into a core [`CustomFormat`].
///
/// # Errors
/// Returns [`DecideError::UnsupportedTrashSpec`] for an unrecognized spec.
pub fn convert(
    cf: TrashCustomFormat,
    scores: &HashMap<String, i32>,
) -> Result<CustomFormat, DecideError> {
    let score = cf
        .trash_id
        .as_ref()
        .and_then(|id| scores.get(id).copied())
        .unwrap_or(0);

    let mut conditions = Vec::with_capacity(cf.specifications.len());
    for spec in &cf.specifications {
        conditions.push(convert_spec(&cf.name, spec)?);
    }

    Ok(CustomFormat {
        id: CustomFormatId::new(),
        name: cf.name,
        conditions,
        score,
    })
}

fn convert_spec(format_name: &str, spec: &TrashSpecification) -> Result<Condition, DecideError> {
    let kind = match spec.implementation.as_str() {
        "ReleaseTitleSpecification" | "ReleaseTitle" => ConditionKind::ReleaseTitle {
            pattern: value_str(spec),
        },
        "ReleaseGroupSpecification" | "ReleaseGroup" => ConditionKind::ReleaseGroup {
            name: value_str(spec),
        },
        "SourceSpecification" | "Source" => ConditionKind::Source {
            source: source_from_index(spec),
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

/// TRaSH encodes Source as an enum index. Map the common indices clean-room; an
/// unknown index falls back to the lowest source so it never over-matches.
fn source_from_index(spec: &TrashSpecification) -> Source {
    match value_i64(spec) {
        Some(1) => Source::Cam,
        Some(2) => Source::Sdtv,
        Some(3) => Source::Hdtv,
        Some(4) => Source::Webrip,
        Some(5) => Source::WebDl,
        Some(6) => Source::Dvd,
        Some(7) => Source::Bluray,
        Some(8) => Source::Remux,
        _ => Source::Cam,
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
