//! Table-driven tests over the `corpus/scoring/*.toml` vectors.
//!
//! The corpus is the language-neutral spec for the decision engine (see
//! `corpus/README.md`). This harness deserializes every vector and runs it
//! through the public API, so adding a vector adds a test with no code change.

use std::path::PathBuf;

use cellarr_core::{
    Condition, ContentId, ContentRef, Coordinates, CustomFormat, CustomFormatId, MediaFileId,
    MediaType, ParsedRelease, ProperRepack, Protocol, QualityProfile, QualityProfileId,
    QualityRanking, Release, Resolution, Source, Verdict,
};
use cellarr_decide::{decide, score, DecisionContext, MatchContext, OnDiskFile};
use serde::Deserialize;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("scoring")
}

fn load<T: serde::de::DeserializeOwned>(file: &str) -> T {
    let path = corpus_dir().join(file);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    toml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

// --- Shared vector pieces ---------------------------------------------------

#[derive(Debug, Deserialize)]
struct ParsedSpec {
    source: Option<Source>,
    resolution: Option<Resolution>,
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    proper: Option<ProperRepack>,
}

impl ParsedSpec {
    fn build(&self, title: &str) -> ParsedRelease {
        let mut p = ParsedRelease::new(title);
        p.source = self.source;
        p.resolution = self.resolution;
        p.group = self.group.clone();
        p.languages = self.languages.clone();
        p.proper_repack = self.proper;
        p
    }
}

#[derive(Debug, Deserialize)]
struct FormatSpec {
    #[serde(default)]
    name: String,
    score: i32,
    #[serde(default, rename = "condition")]
    conditions: Vec<Condition>,
}

impl FormatSpec {
    fn build(&self) -> CustomFormat {
        CustomFormat {
            id: CustomFormatId::new(),
            name: self.name.clone(),
            conditions: self.conditions.clone(),
            score: self.score,
        }
    }
}

fn release(title: &str, flags: &[String], size: Option<u64>) -> Release {
    Release {
        indexer_id: cellarr_core::IndexerId::new(),
        title: title.to_string(),
        download_url: "magnet:?xt=urn:test".to_string(),
        guid: None,
        protocol: Protocol::Torrent,
        size,
        seeders: None,
        indexer_flags: flags.to_vec(),
    }
}

// --- Match vectors ----------------------------------------------------------

#[derive(Debug, Deserialize)]
struct MatchFile {
    case: Vec<MatchCase>,
}

#[derive(Debug, Deserialize)]
struct MatchCase {
    name: String,
    title: String,
    #[serde(default)]
    flags: Vec<String>,
    #[serde(default)]
    size: Option<u64>,
    parsed: ParsedSpec,
    format: FormatSpec,
    expected: MatchExpected,
}

#[derive(Debug, Deserialize)]
struct MatchExpected {
    matches: bool,
}

#[test]
fn cf_match_vectors() {
    let file: MatchFile = load("cf_match.toml");
    assert!(!file.case.is_empty(), "no match vectors loaded");
    for case in &file.case {
        let parsed = case.parsed.build(&case.title);
        let rel = release(&case.title, &case.flags, case.size);
        let format = case.format.build();
        let formats = vec![format.clone()];
        let ctx = MatchContext::new(&formats).expect("compile regexes");
        let got = ctx.matches(&format, &rel, &parsed);
        assert_eq!(
            got, case.expected.matches,
            "match vector '{}': expected {}, got {}",
            case.name, case.expected.matches, got
        );
    }
}

// --- Score vectors ----------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ScoreFile {
    case: Vec<ScoreCase>,
}

#[derive(Debug, Deserialize)]
struct ScoreCase {
    name: String,
    title: String,
    #[serde(default)]
    flags: Vec<String>,
    #[serde(default)]
    size: Option<u64>,
    parsed: ParsedSpec,
    #[serde(default, rename = "format")]
    formats: Vec<FormatSpec>,
    expected: ScoreExpected,
}

#[derive(Debug, Deserialize)]
struct ScoreExpected {
    score: i32,
}

#[test]
fn cf_score_vectors() {
    let file: ScoreFile = load("cf_score.toml");
    assert!(!file.case.is_empty(), "no score vectors loaded");
    for case in &file.case {
        let parsed = case.parsed.build(&case.title);
        let rel = release(&case.title, &case.flags, case.size);
        let formats: Vec<CustomFormat> = case.formats.iter().map(FormatSpec::build).collect();
        let ctx = MatchContext::new(&formats).expect("compile regexes");
        let got = score(&rel, &parsed, &formats, &ctx);
        assert_eq!(
            got, case.expected.score,
            "score vector '{}': expected {}, got {}",
            case.name, case.expected.score, got
        );
    }
}

// --- Decision vectors -------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DecisionFile {
    case: Vec<DecisionCase>,
}

#[derive(Debug, Deserialize)]
struct DecisionCase {
    name: String,
    title: String,
    #[serde(default)]
    flags: Vec<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    blocklisted: bool,
    parsed: ParsedSpec,
    profile: ProfileSpec,
    #[serde(default, rename = "format")]
    formats: Vec<FormatSpec>,
    #[serde(default)]
    on_disk: Option<OnDiskSpec>,
    expected: DecisionExpected,
}

#[derive(Debug, Deserialize)]
struct ProfileSpec {
    allowed: Vec<u32>,
    upgrades_allowed: bool,
    cutoff_quality: u32,
    min_cf_score: i32,
    upgrade_until_cf_score: i32,
    #[serde(default)]
    required_languages: Vec<String>,
}

impl ProfileSpec {
    fn build(&self) -> QualityProfile {
        QualityProfile {
            id: QualityProfileId::new(),
            name: "test".to_string(),
            allowed_qualities: self.allowed.clone(),
            upgrades_allowed: self.upgrades_allowed,
            cutoff_quality: self.cutoff_quality,
            min_custom_format_score: self.min_cf_score,
            upgrade_until_custom_format_score: self.upgrade_until_cf_score,
            required_languages: self.required_languages.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct OnDiskSpec {
    quality_rank: u32,
    cf_score: i32,
}

#[derive(Debug, Deserialize)]
struct DecisionExpected {
    verdict: String,
    #[serde(default)]
    reason: Option<String>,
}

fn content_ref() -> ContentRef {
    ContentRef::new(
        ContentId::new(),
        cellarr_core::LibraryId::new(),
        MediaType::Movie,
        Coordinates::Movie,
    )
    .expect("valid movie content ref")
}

#[test]
fn decision_vectors() {
    let file: DecisionFile = load("decision.toml");
    assert!(!file.case.is_empty(), "no decision vectors loaded");
    let ranking = QualityRanking::default();

    for case in &file.case {
        let parsed = case.parsed.build(&case.title);
        let rel = release(&case.title, &case.flags, case.size);
        let formats: Vec<CustomFormat> = case.formats.iter().map(FormatSpec::build).collect();
        let profile = case.profile.build();
        let on_disk = case.on_disk.as_ref().map(|d| OnDiskFile {
            file_id: MediaFileId::new(),
            quality_rank: d.quality_rank,
            custom_format_score: d.cf_score,
            release_type: None,
        });

        let ctx = DecisionContext {
            profile: &profile,
            custom_formats: &formats,
            ranking: &ranking,
            blocklisted: case.blocklisted,
            proper_repack_policy: Default::default(),
            indexer_criteria: Default::default(),
            indexer_priority: 0,
            content_runtime: None,
        };

        let decision =
            decide(content_ref(), &rel, &parsed, on_disk, &ctx).expect("decision is infallible");

        let (verdict_kind, reason_kind) = describe(&decision.verdict);
        assert_eq!(
            verdict_kind, case.expected.verdict,
            "decision vector '{}': expected verdict {}, got {} (full: {:?})",
            case.name, case.expected.verdict, verdict_kind, decision.verdict
        );
        if let Some(expected_reason) = &case.expected.reason {
            assert_eq!(
                reason_kind.as_deref(),
                Some(expected_reason.as_str()),
                "decision vector '{}': expected reason {:?}, got {:?}",
                case.name,
                expected_reason,
                reason_kind
            );
        }
    }
}

/// Map a verdict to (verdict_kind, reason_kind) snake_case strings matching the
/// corpus's `expected.verdict` / `expected.reason`.
fn describe(verdict: &Verdict) -> (String, Option<String>) {
    match verdict {
        Verdict::Grab { .. } => ("grab".to_string(), None),
        Verdict::Upgrade { .. } => ("upgrade".to_string(), None),
        Verdict::Reject { reason } => {
            // Serialize the reason and read back its snake_case tag.
            let value = serde_json::to_value(reason).expect("reason serializes");
            let tag = value
                .get("reason")
                .and_then(|r| r.as_str())
                .map(str::to_string);
            ("reject".to_string(), tag)
        }
    }
}
