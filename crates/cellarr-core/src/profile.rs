//! Quality profiles and custom formats.
//!
//! These types model the TRaSH-compatible decision vocabulary: a global quality
//! ranking, per-library profiles, and named custom formats built from
//! conditions. The *scoring and decision arithmetic* lives in `cellarr-decide`;
//! core owns the data model and the pure per-condition matching semantics
//! (OR by default, `required` = AND, `negate` = absence), because those
//! semantics are part of the shared vocabulary and are cheaply unit-testable.

use serde::{Deserialize, Serialize};

use crate::ids::{CustomFormatId, QualityProfileId};
use crate::parsed::{HdrFormat, ParsedRelease, ProperRepack, Resolution, Source, VideoCodec};

/// A named, ordered quality (a position in the global worst→best ranking).
///
/// The numeric `rank` is the authoritative ordering used by the decision engine;
/// higher means better. Sizes are advisory bounds in bytes-per-minute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityDefinition {
    /// Stable name (e.g. "Bluray-1080p").
    pub name: String,
    /// Position in the global ranking; higher is better.
    pub rank: u32,
    /// Minimum advisory size, bytes per minute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_size_per_min: Option<u64>,
    /// Maximum advisory size, bytes per minute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size_per_min: Option<u64>,
}

/// A user's allowed qualities, ordering, cutoff, and CF-score thresholds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityProfile {
    /// Profile identifier.
    pub id: QualityProfileId,
    /// Human-facing name.
    pub name: String,
    /// Allowed quality ranks, in the user's preferred order (best first or last
    /// is a presentation choice; the engine uses [`QualityDefinition::rank`]).
    pub allowed_qualities: Vec<u32>,
    /// Whether upgrades are permitted at all.
    pub upgrades_allowed: bool,
    /// The quality rank at which upgrading stops.
    pub cutoff_quality: u32,
    /// Reject anything below this total custom-format score.
    pub min_custom_format_score: i32,
    /// Stop chasing custom-format score once this total is reached.
    pub upgrade_until_custom_format_score: i32,
    /// Required language codes; empty means no language requirement.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_languages: Vec<String>,
}

/// The kinds of facts a custom-format condition can test.
///
/// The schema is a superset-compatible match for TRaSH's so community
/// definitions import losslessly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConditionKind {
    /// A regex tested against the raw release title.
    ReleaseTitle {
        /// The pattern (evaluated by the decision engine, which owns the regex
        /// dependency; core only models it).
        pattern: String,
    },
    /// An exact release-group match (case-insensitive).
    ReleaseGroup {
        /// The group name.
        name: String,
    },
    /// A source/medium match.
    Source {
        /// The required source.
        source: Source,
    },
    /// A resolution match.
    Resolution {
        /// The required resolution.
        resolution: Resolution,
    },
    /// A video-codec match.
    Codec {
        /// The required codec.
        codec: VideoCodec,
    },
    /// An HDR-format match.
    Hdr {
        /// The required HDR format.
        format: HdrFormat,
    },
    /// A proper/repack modifier match.
    QualityModifier {
        /// The required modifier.
        modifier: ProperRepack,
    },
    /// A language match (code or name, case-insensitive).
    Language {
        /// The required language.
        language: String,
    },
    /// An indexer-flag match (e.g. "freeleech"), case-insensitive.
    IndexerFlag {
        /// The required flag.
        flag: String,
    },
    /// A size-range match, in bytes.
    Size {
        /// Inclusive minimum.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<u64>,
        /// Inclusive maximum.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<u64>,
    },
}

/// One condition within a custom format, with its TRaSH-compatible modifiers.
///
/// - `required = true` makes the condition mandatory (logical **AND**).
/// - `negate = true` makes the condition match on **absence** of the fact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    /// What the condition tests.
    #[serde(flatten)]
    pub kind: ConditionKind,
    /// When true, this condition must match (AND semantics).
    #[serde(default)]
    pub required: bool,
    /// When true, the condition matches when the fact is absent.
    #[serde(default)]
    pub negate: bool,
}

/// A named bundle of conditions carrying a score.
///
/// A release matches a custom format when, after applying `negate` to each
/// condition's raw result: every `required` condition matches **and** at least
/// one non-required condition matches (or there are no non-required conditions).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomFormat {
    /// Format identifier.
    pub id: CustomFormatId,
    /// Human-facing name.
    pub name: String,
    /// The conditions that define the format.
    pub conditions: Vec<Condition>,
    /// The score contributed when the format matches (may be negative).
    pub score: i32,
}

/// Evaluate one condition's *raw* fact against a parsed release, **before**
/// `negate` is applied.
///
/// Regex-based [`ConditionKind::ReleaseTitle`] is intentionally **not** decided
/// here: core has no regex dependency, so the caller must supply the raw result
/// for title patterns. This keeps core pure while still owning the boolean
/// algebra (required/negate/OR) that defines the semantics.
///
/// `title_regex_result` is consulted only for [`ConditionKind::ReleaseTitle`];
/// pass `None` for other kinds. When a title condition is evaluated with
/// `None`, it is treated as not matching.
fn raw_condition_matches(
    kind: &ConditionKind,
    parsed: &ParsedRelease,
    title_regex_result: Option<bool>,
) -> bool {
    match kind {
        ConditionKind::ReleaseTitle { .. } => title_regex_result.unwrap_or(false),
        ConditionKind::ReleaseGroup { name } => parsed
            .group
            .as_deref()
            .is_some_and(|g| g.eq_ignore_ascii_case(name)),
        ConditionKind::Source { source } => parsed.source == Some(*source),
        ConditionKind::Resolution { resolution } => parsed.resolution == Some(*resolution),
        ConditionKind::Codec { codec } => parsed.codec == Some(*codec),
        ConditionKind::Hdr { format } => parsed.hdr.contains(format),
        ConditionKind::QualityModifier { modifier } => parsed.proper_repack == Some(*modifier),
        ConditionKind::Language { language } => parsed
            .languages
            .iter()
            .any(|l| l.eq_ignore_ascii_case(language)),
        // Indexer flags and size are not carried on ParsedRelease (they live on
        // the Release); a parse-only evaluation cannot confirm them, so they are
        // treated as not matching here. Full evaluation against a Release lives
        // in cellarr-decide.
        ConditionKind::IndexerFlag { .. } | ConditionKind::Size { .. } => false,
    }
}

/// The effective result of a condition: its raw match XORed with `negate`.
#[must_use]
pub fn condition_matches(
    condition: &Condition,
    parsed: &ParsedRelease,
    title_regex_result: Option<bool>,
) -> bool {
    let raw = raw_condition_matches(&condition.kind, parsed, title_regex_result);
    raw ^ condition.negate
}

/// Whether a custom format matches a parsed release, applying the TRaSH boolean
/// algebra: all `required` conditions must match, and at least one non-required
/// condition must match (vacuously true when there are no non-required
/// conditions).
///
/// `title_regex_results` supplies, in `conditions` order, the precomputed regex
/// outcome for each [`ConditionKind::ReleaseTitle`] condition; non-title
/// conditions ignore it. A shorter slice (or `None` entries) treats missing
/// title results as non-matching.
#[must_use]
pub fn custom_format_matches(
    format: &CustomFormat,
    parsed: &ParsedRelease,
    title_regex_results: &[Option<bool>],
) -> bool {
    let mut all_required = true;
    let mut any_optional = false;
    let mut have_optional = false;

    for (idx, condition) in format.conditions.iter().enumerate() {
        let regex_result = title_regex_results.get(idx).copied().flatten();
        let matched = condition_matches(condition, parsed, regex_result);
        if condition.required {
            all_required &= matched;
        } else {
            have_optional = true;
            any_optional |= matched;
        }
    }

    all_required && (any_optional || !have_optional)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsed::ParsedRelease;

    fn parsed_with_group_and_source() -> ParsedRelease {
        let mut p = ParsedRelease::new("Show.S01E01.1080p.BluRay.x264-GROUP");
        p.group = Some("GROUP".to_string());
        p.source = Some(Source::Bluray);
        p.resolution = Some(Resolution::R1080p);
        p
    }

    fn cf(name: &str, conditions: Vec<Condition>, score: i32) -> CustomFormat {
        CustomFormat {
            id: CustomFormatId::new(),
            name: name.to_string(),
            conditions,
            score,
        }
    }

    fn cond(kind: ConditionKind, required: bool, negate: bool) -> Condition {
        Condition {
            kind,
            required,
            negate,
        }
    }

    #[test]
    fn or_default_matches_when_any_optional_condition_matches() {
        let p = parsed_with_group_and_source();
        let format = cf(
            "or",
            vec![
                cond(
                    ConditionKind::Source {
                        source: Source::WebDl,
                    },
                    false,
                    false,
                ),
                cond(
                    ConditionKind::Source {
                        source: Source::Bluray,
                    },
                    false,
                    false,
                ),
            ],
            100,
        );
        assert!(custom_format_matches(&format, &p, &[]));
    }

    #[test]
    fn or_default_fails_when_no_optional_condition_matches() {
        let p = parsed_with_group_and_source();
        let format = cf(
            "or-none",
            vec![cond(
                ConditionKind::Source {
                    source: Source::WebDl,
                },
                false,
                false,
            )],
            100,
        );
        assert!(!custom_format_matches(&format, &p, &[]));
    }

    #[test]
    fn required_acts_as_and() {
        let p = parsed_with_group_and_source();
        // Both required; one fails -> whole format fails.
        let format = cf(
            "and",
            vec![
                cond(
                    ConditionKind::Source {
                        source: Source::Bluray,
                    },
                    true,
                    false,
                ),
                cond(
                    ConditionKind::Resolution {
                        resolution: Resolution::R2160p,
                    },
                    true,
                    false,
                ),
            ],
            100,
        );
        assert!(!custom_format_matches(&format, &p, &[]));

        // Both required and both present -> matches.
        let format_ok = cf(
            "and-ok",
            vec![
                cond(
                    ConditionKind::Source {
                        source: Source::Bluray,
                    },
                    true,
                    false,
                ),
                cond(
                    ConditionKind::Resolution {
                        resolution: Resolution::R1080p,
                    },
                    true,
                    false,
                ),
            ],
            100,
        );
        assert!(custom_format_matches(&format_ok, &p, &[]));
    }

    #[test]
    fn negate_matches_on_absence() {
        let p = parsed_with_group_and_source();
        // Negated, optional: "source is NOT WEB-DL" -> true because source is BluRay.
        let format = cf(
            "neg",
            vec![cond(
                ConditionKind::Source {
                    source: Source::WebDl,
                },
                false,
                true,
            )],
            100,
        );
        assert!(custom_format_matches(&format, &p, &[]));

        // Negated, optional: "source is NOT BluRay" -> false because it IS BluRay.
        let format2 = cf(
            "neg2",
            vec![cond(
                ConditionKind::Source {
                    source: Source::Bluray,
                },
                false,
                true,
            )],
            100,
        );
        assert!(!custom_format_matches(&format2, &p, &[]));
    }

    #[test]
    fn required_negate_is_mandatory_absence() {
        let p = parsed_with_group_and_source();
        // Required + negate: "must NOT be a CAM". Present source is BluRay, so the
        // CAM fact is absent -> required condition satisfied. No optional
        // conditions -> the format matches.
        let format = cf(
            "no-cam",
            vec![cond(
                ConditionKind::Source {
                    source: Source::Cam,
                },
                true,
                true,
            )],
            -10000,
        );
        assert!(custom_format_matches(&format, &p, &[]));
    }

    #[test]
    fn title_regex_result_is_consumed_positionally() {
        let p = parsed_with_group_and_source();
        let format = cf(
            "title",
            vec![cond(
                ConditionKind::ReleaseTitle {
                    pattern: "(?i)proper".to_string(),
                },
                false,
                false,
            )],
            50,
        );
        assert!(custom_format_matches(&format, &p, &[Some(true)]));
        assert!(!custom_format_matches(&format, &p, &[Some(false)]));
        // Missing result defaults to non-matching.
        assert!(!custom_format_matches(&format, &p, &[]));
    }
}
