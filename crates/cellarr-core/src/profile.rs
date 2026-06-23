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

/// A resolved quality: the concrete name plus its position in the global
/// ranking.
///
/// This is the value stored on a [`crate::media::MediaFile`] and produced by
/// [`resolve_quality`] from a parse. It is the bridge between the
/// [`QualityDefinition`] *catalogue* (names ↔ ranks) and a specific file: the
/// `rank` is the authoritative ordering the decision engine compares (higher is
/// better), and `name` is what the UI and logs display.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Quality {
    /// Stable name (matches a [`QualityDefinition::name`], e.g. "Bluray-1080p").
    pub name: String,
    /// Position in the global ranking; higher is better. Mirrors the matching
    /// [`QualityDefinition::rank`].
    pub rank: u32,
}

impl Quality {
    /// Construct a quality from a name and rank.
    #[must_use]
    pub fn new(name: impl Into<String>, rank: u32) -> Self {
        Self {
            name: name.into(),
            rank,
        }
    }
}

/// The default global quality ranking, worst → best.
///
/// Names follow the TRaSH / *arr convention (`<Source>-<Resolution>`, with
/// `Remux` and `WEBRip`/`WEBDL` spelled out). Ranks are dense and ascending so
/// `rank` alone orders any two qualities. This is the *default*; a deployment can
/// override it (see [`QualityRanking`]) but ships with this sane baseline.
///
/// The list is intentionally explicit rather than computed so it reads like the
/// user-facing quality table and is trivial to diff when the catalogue changes.
/// The remux buckets keep the Sonarr spelling (`Bluray-<res> Remux`) as the
/// single canonical internal name; the Radarr face renames them to `Remux-<res>`
/// in the `/api/v3` shim. The pre-retail movie tiers (`WORKPRINT`, `TELESYNC`,
/// `TELECINE`, `REGIONAL`, `DVDSCR`, `DVD-R`) and the full-disc/Raw buckets
/// (`BR-DISK`, `Raw-HD`) are placed in Radarr's canonical worst→best weight
/// order (its catalogue is the superset that contains every bucket). `Bluray-576p`
/// and `Raw-HD` exist in both apps. This keeps the prior ordering as a subsequence
/// so no existing relative ranking changes.
const DEFAULT_QUALITY_NAMES: &[&str] = &[
    "Unknown",
    "WORKPRINT",
    "CAM",
    "TELESYNC",
    "TELECINE",
    "REGIONAL",
    "DVDSCR",
    "SDTV",
    "DVD",
    "DVD-R",
    "WEBRip-480p",
    "WEBDL-480p",
    "Bluray-480p",
    "Bluray-576p",
    "HDTV-720p",
    "WEBRip-720p",
    "WEBDL-720p",
    "Bluray-720p",
    "HDTV-1080p",
    "WEBRip-1080p",
    "WEBDL-1080p",
    "Bluray-1080p",
    "Bluray-1080p Remux",
    "HDTV-2160p",
    "WEBRip-2160p",
    "WEBDL-2160p",
    "Bluray-2160p",
    "Bluray-2160p Remux",
    "BR-DISK",
    "Raw-HD",
];

/// An ordered quality catalogue: the names that exist and their ranks.
///
/// Wraps the worst→best ordering so callers can resolve a parse to a [`Quality`]
/// against either the shipped default ([`QualityRanking::default`]) or a custom
/// ranking supplied by configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityRanking {
    /// Quality definitions in worst→best order (index is *not* the rank; the
    /// authoritative rank is [`QualityDefinition::rank`]).
    pub qualities: Vec<QualityDefinition>,
}

impl Default for QualityRanking {
    fn default() -> Self {
        let qualities = DEFAULT_QUALITY_NAMES
            .iter()
            .enumerate()
            .map(|(idx, name)| QualityDefinition {
                name: (*name).to_string(),
                // Dense ascending ranks; index order is worst→best.
                rank: idx as u32,
                min_size_per_min: None,
                max_size_per_min: None,
            })
            .collect();
        Self { qualities }
    }
}

impl QualityRanking {
    /// Look up a [`Quality`] by its catalogue name, if present.
    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<Quality> {
        self.qualities
            .iter()
            .find(|q| q.name.eq_ignore_ascii_case(name))
            .map(|q| Quality::new(q.name.clone(), q.rank))
    }

    /// The sentinel "Unknown" quality (rank 0 in the default ranking), used when
    /// a parse carries no recognizable source/resolution.
    #[must_use]
    fn unknown(&self) -> Quality {
        self.by_name("Unknown")
            .unwrap_or_else(|| Quality::new("Unknown", 0))
    }
}

/// Map a parsed release's source + resolution (+ remux/proper signals) to a
/// [`Quality`] in `ranking`.
///
/// The mapping mirrors how the *arr stack derives a quality bucket from the two
/// independent axes a release advertises (the *medium* and the *resolution*):
///
/// - The medium (`Source`) selects the family — CAM, SDTV, DVD, WEBRip, WEB-DL,
///   Bluray — and, for Bluray, whether it is a `Remux`.
/// - The resolution selects the tier within the family (720p/1080p/2160p; SD
///   media collapse to their family name).
///
/// `Remux` source is treated as Bluray-Remux at its resolution. The proper/repack
/// signal does **not** change the quality bucket (it is a same-quality upgrade
/// modifier handled by the decision engine), so it is not consulted here; it is
/// named in the signature for forward-compatibility and to document intent.
///
/// Falls back to the catalogue's `Unknown` when the parse lacks the signals
/// needed to name a bucket.
#[must_use]
pub fn resolve_quality(parsed: &ParsedRelease, ranking: &QualityRanking) -> Quality {
    let name = match (parsed.source, parsed.resolution) {
        // Pre-retail movie tiers and full-disc/raw buckets are each their own
        // bucket regardless of advertised resolution (they mirror Radarr's
        // quality catalogue, which keys these by source alone).
        (Some(Source::Workprint), _) => "WORKPRINT",
        (Some(Source::Cam), _) => "CAM",
        (Some(Source::Telesync), _) => "TELESYNC",
        (Some(Source::Telecine), _) => "TELECINE",
        (Some(Source::Regional), _) => "REGIONAL",
        (Some(Source::Dvdscr), _) => "DVDSCR",
        (Some(Source::DvdR), _) => "DVD-R",
        (Some(Source::RawHd), _) => "Raw-HD",
        // A full untouched Blu-ray/UHD disc collapses to the single BR-DISK
        // bucket regardless of resolution (matches Radarr).
        (Some(Source::BrDisk), _) => "BR-DISK",
        (Some(Source::Sdtv), _) => "SDTV",
        (Some(Source::Dvd), _) => "DVD",

        (Some(Source::Hdtv), Some(Resolution::R720p)) => "HDTV-720p",
        (Some(Source::Hdtv), Some(Resolution::R1080p)) => "HDTV-1080p",
        (Some(Source::Hdtv), Some(Resolution::R2160p)) => "HDTV-2160p",

        (Some(Source::Webrip), Some(Resolution::R480p | Resolution::R576p)) => "WEBRip-480p",
        (Some(Source::Webrip), Some(Resolution::R720p)) => "WEBRip-720p",
        (Some(Source::Webrip), Some(Resolution::R1080p)) => "WEBRip-1080p",
        (Some(Source::Webrip), Some(Resolution::R2160p)) => "WEBRip-2160p",

        (Some(Source::WebDl), Some(Resolution::R480p | Resolution::R576p)) => "WEBDL-480p",
        (Some(Source::WebDl), Some(Resolution::R720p)) => "WEBDL-720p",
        (Some(Source::WebDl), Some(Resolution::R1080p)) => "WEBDL-1080p",
        (Some(Source::WebDl), Some(Resolution::R2160p)) => "WEBDL-2160p",

        // A bare Remux source implies Bluray-Remux at its resolution.
        (Some(Source::Remux), Some(Resolution::R2160p)) => "Bluray-2160p Remux",
        (Some(Source::Remux), _) => "Bluray-1080p Remux",

        (Some(Source::Bluray), Some(Resolution::R480p | Resolution::R576p)) => "Bluray-480p",
        (Some(Source::Bluray), Some(Resolution::R720p)) => "Bluray-720p",
        (Some(Source::Bluray), Some(Resolution::R2160p)) => "Bluray-2160p",
        // Bluray with 1080p or unknown resolution collapses to Bluray-1080p, the
        // conventional default for an unqualified Bluray.
        (Some(Source::Bluray), _) => "Bluray-1080p",

        // HDTV with no resolution, or sub-HD HDTV, is SD broadcast (matches the
        // originals: `...HDTV.x264` → SDTV; `480p.HDTV` → SDTV).
        (Some(Source::Hdtv), None | Some(Resolution::R480p | Resolution::R576p)) => "SDTV",

        // Source-less, resolution-only releases (common in anime and raw web): the
        // originals default the medium to HDTV at the advertised resolution, with
        // sub-HD collapsing to SDTV.
        (None, Some(Resolution::R480p | Resolution::R576p)) => "SDTV",
        (None, Some(Resolution::R720p)) => "HDTV-720p",
        (None, Some(Resolution::R1080p)) => "HDTV-1080p",
        (None, Some(Resolution::R2160p)) => "HDTV-2160p",

        // WEBRip / WEB-DL with no resolution cannot be bucketed; neither can a
        // release with no source and no resolution.
        _ => return ranking.unknown(),
    };

    ranking.by_name(name).unwrap_or_else(|| ranking.unknown())
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

    fn parsed_with(source: Option<Source>, resolution: Option<Resolution>) -> ParsedRelease {
        let mut p = ParsedRelease::new("test");
        p.source = source;
        p.resolution = resolution;
        p
    }

    #[test]
    fn default_ranking_orders_worst_to_best() {
        let r = QualityRanking::default();
        let sdtv = r.by_name("SDTV").expect("SDTV present");
        let bluray_1080p = r.by_name("Bluray-1080p").expect("Bluray-1080p present");
        let bluray_2160p_remux = r
            .by_name("Bluray-2160p Remux")
            .expect("Bluray-2160p Remux present");
        assert!(sdtv.rank < bluray_1080p.rank);
        assert!(bluray_1080p.rank < bluray_2160p_remux.rank);
    }

    #[test]
    fn resolve_quality_maps_representative_releases() {
        let r = QualityRanking::default();

        let webdl_1080p = resolve_quality(
            &parsed_with(Some(Source::WebDl), Some(Resolution::R1080p)),
            &r,
        );
        assert_eq!(webdl_1080p.name, "WEBDL-1080p");

        let bluray_remux_2160 = resolve_quality(
            &parsed_with(Some(Source::Remux), Some(Resolution::R2160p)),
            &r,
        );
        assert_eq!(bluray_remux_2160.name, "Bluray-2160p Remux");

        // A bare Bluray with no resolution defaults to Bluray-1080p.
        let bluray_default = resolve_quality(&parsed_with(Some(Source::Bluray), None), &r);
        assert_eq!(bluray_default.name, "Bluray-1080p");

        // Remux outranks plain Bluray at the same resolution.
        let plain = resolve_quality(
            &parsed_with(Some(Source::Bluray), Some(Resolution::R1080p)),
            &r,
        );
        let remux = resolve_quality(
            &parsed_with(Some(Source::Remux), Some(Resolution::R1080p)),
            &r,
        );
        assert!(remux.rank > plain.rank);
    }

    #[test]
    fn resolve_quality_falls_back_to_unknown() {
        let r = QualityRanking::default();
        // Genuinely unbucketable: no source and no resolution.
        let unknown = resolve_quality(&parsed_with(None, None), &r);
        assert_eq!(unknown.name, "Unknown");
        assert_eq!(unknown.rank, 0);
    }

    #[test]
    fn resolve_quality_defaults_resolution_only_to_hdtv() {
        // Source-less, resolution-only releases (anime/web raws) match the
        // originals: HDTV at the advertised resolution, sub-HD → SDTV. (Parity G1.)
        let r = QualityRanking::default();
        assert_eq!(
            resolve_quality(&parsed_with(None, Some(Resolution::R1080p)), &r).name,
            "HDTV-1080p"
        );
        assert_eq!(
            resolve_quality(&parsed_with(None, Some(Resolution::R720p)), &r).name,
            "HDTV-720p"
        );
        assert_eq!(
            resolve_quality(&parsed_with(None, Some(Resolution::R480p)), &r).name,
            "SDTV"
        );
    }

    #[test]
    fn resolve_quality_hdtv_without_resolution_is_sdtv() {
        // `...HDTV.x264` with no resolution token → SDTV. (Parity G2.)
        let r = QualityRanking::default();
        assert_eq!(
            resolve_quality(&parsed_with(Some(Source::Hdtv), None), &r).name,
            "SDTV"
        );
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
