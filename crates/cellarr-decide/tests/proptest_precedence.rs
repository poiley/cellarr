//! Property tests for the decision engine's scoring and precedence contract.
//!
//! These pin the *invariants* of `docs/05-decision-engine.md` across randomly
//! generated profiles, releases, on-disk files, and custom formats — not just the
//! hand-picked vectors in `corpus.rs`. Each property restates one rule of the
//! precedence contract and asserts it holds for the whole generated space:
//!
//! 1. Scoring monotonicity: adding a positive-score custom format that matches
//!    never lowers the total CF score.
//! 2. Quality dominates CF: a strictly *worse* quality candidate is never an
//!    upgrade over an existing file, regardless of how high its CF score is — the
//!    engine never downgrades quality to chase CF score.
//! 3. Cutoff stop: once BOTH cutoffs (quality and CF) are met by the on-disk
//!    file, an equal-or-worse candidate is rejected as cutoff-met (no churn).
//!
//! A fourth, fuzz-style property throws arbitrary byte-ish scene-name fragments
//! at the full parse→match→decide path and asserts it is total (never panics).

use cellarr_core::{
    Condition, ConditionKind, ContentId, ContentRef, Coordinates, CustomFormat, CustomFormatId,
    IndexerId, LibraryId, MediaFileId, MediaType, ParsedRelease, Protocol, QualityProfile,
    QualityProfileId, QualityRanking, Release, Resolution, Source, Verdict,
};
use cellarr_decide::{
    decide, score, DecisionContext, MatchContext, OnDiskFile, ProperRepackPolicy,
};
use proptest::prelude::*;

// --- generators ----------------------------------------------------------

/// A small set of (source, resolution) pairs spanning a range of ranks so the
/// generated candidate qualities are real, comparable catalogue entries.
fn quality_axes() -> impl Strategy<Value = (Source, Resolution)> {
    prop_oneof![
        Just((Source::Webrip, Resolution::R720p)),
        Just((Source::WebDl, Resolution::R720p)),
        Just((Source::WebDl, Resolution::R1080p)),
        Just((Source::Bluray, Resolution::R1080p)),
        Just((Source::WebDl, Resolution::R2160p)),
        Just((Source::Bluray, Resolution::R2160p)),
    ]
}

fn parsed(source: Source, resolution: Resolution) -> ParsedRelease {
    let mut p = ParsedRelease::new("t");
    p.source = Some(source);
    p.resolution = Some(resolution);
    p
}

fn release(flags: &[String]) -> Release {
    Release {
        indexer_id: IndexerId::new(),
        title: "Some.Release.1080p.WEB-DL-GRP".to_string(),
        download_url: "magnet:?xt=urn:test".to_string(),
        guid: None,
        protocol: Protocol::Torrent,
        size: None,
        seeders: None,
        indexer_flags: flags.to_vec(),
    }
}

fn content_ref() -> ContentRef {
    ContentRef::new(
        ContentId::new(),
        LibraryId::new(),
        MediaType::Movie,
        Coordinates::Movie,
    )
    .unwrap()
}

/// A custom format that matches when the release carries the named indexer flag.
fn flag_format(name: &str, flag: &str, score: i32) -> CustomFormat {
    CustomFormat {
        id: CustomFormatId::new(),
        name: name.to_string(),
        conditions: vec![Condition {
            kind: ConditionKind::IndexerFlag {
                flag: flag.to_string(),
            },
            required: false,
            negate: false,
        }],
        score,
    }
}

/// All allowed ranks (the full default catalogue), so a generated candidate
/// quality is never rejected merely for being disallowed.
fn all_ranks(ranking: &QualityRanking) -> Vec<u32> {
    ranking.qualities.iter().map(|q| q.rank).collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// Scoring monotonicity: adding a *matching* custom format with a
    /// non-negative score never lowers the total. The score is a sum, so a
    /// positive addend can only raise or hold it.
    #[test]
    fn adding_a_positive_matching_cf_never_lowers_the_total(
        base_score in -500i32..500,
        extra_score in 0i32..2000,
    ) {
        let flags = vec!["freeleech".to_string(), "internal".to_string()];
        let rel = release(&flags);
        let p = parsed(Source::WebDl, Resolution::R1080p);

        // Baseline: a single CF matching `freeleech`.
        let base = vec![flag_format("freeleech", "freeleech", base_score)];
        let ctx_base = MatchContext::new(&base).unwrap();
        let total_base = score(&rel, &p, &base, &ctx_base);

        // Add a second CF (matching `internal`) with a non-negative score.
        let mut more = base.clone();
        more.push(flag_format("internal", "internal", extra_score));
        let ctx_more = MatchContext::new(&more).unwrap();
        let total_more = score(&rel, &p, &more, &ctx_more);

        prop_assert!(
            total_more >= total_base,
            "adding a +{extra_score} matching CF lowered the total: {total_base} -> {total_more}"
        );
    }

    /// Quality dominates CF score: a candidate at a *strictly lower* quality rank
    /// than the on-disk file is never an Upgrade, no matter how large its CF
    /// score. Generated across the full axis matrix and a wide CF score range.
    #[test]
    fn a_strictly_worse_quality_is_never_an_upgrade_even_with_a_huge_cf_score(
        (cand_axes, disk_axes) in (quality_axes(), quality_axes()),
        cand_cf in 0i32..1_000_000,
    ) {
        let ranking = QualityRanking::default();
        let cand_q = cellarr_core::resolve_quality(
            &parsed(cand_axes.0, cand_axes.1),
            &ranking,
        );
        let disk_q = cellarr_core::resolve_quality(
            &parsed(disk_axes.0, disk_axes.1),
            &ranking,
        );
        // Only the strictly-worse-quality case is in scope for this property.
        prop_assume!(cand_q.rank < disk_q.rank);

        // A single freeleech CF worth `cand_cf`, matched by the candidate.
        let formats = vec![flag_format("freeleech", "freeleech", cand_cf)];
        let prof = QualityProfile {
            id: QualityProfileId::new(),
            name: "p".to_string(),
            allowed_qualities: all_ranks(&ranking),
            upgrades_allowed: true,
            // Cutoff above everything so the existing file is below cutoff (the
            // case where an upgrade *would* be allowed if quality didn't dominate).
            cutoff_quality: u32::MAX,
            min_custom_format_score: i32::MIN,
            upgrade_until_custom_format_score: i32::MAX,
            required_languages: vec![],
        };
        let ctx = DecisionContext {
            profile: &prof,
            custom_formats: &formats,
            ranking: &ranking,
            blocklisted: false,
            proper_repack_policy: ProperRepackPolicy::Prefer,
        };
        // On-disk file: the better quality, with CF score 0 (so the candidate's
        // CF score is strictly higher — the temptation the rule must resist).
        let existing = OnDiskFile {
            file_id: MediaFileId::new(),
            quality_rank: disk_q.rank,
            custom_format_score: 0,
            release_type: None,
        };
        let decision = decide(
            content_ref(),
            &release(&["freeleech".to_string()]),
            &parsed(cand_axes.0, cand_axes.1),
            Some(existing),
            &ctx,
        )
        .unwrap();
        prop_assert!(
            !matches!(decision.verdict, Verdict::Upgrade { .. }),
            "a worse-quality candidate (rank {}) with CF {cand_cf} upgraded over rank {}",
            cand_q.rank, disk_q.rank
        );
    }

    /// Cutoff stop: when the on-disk file already meets BOTH the quality cutoff
    /// and the CF cutoff, an equal-quality candidate that does not exceed the CF
    /// score is rejected (CutoffAlreadyMet) — the engine stops churning.
    #[test]
    fn both_cutoffs_met_rejects_an_equal_candidate(
        axes in quality_axes(),
        disk_cf in 100i32..1000,
    ) {
        let ranking = QualityRanking::default();
        let q = cellarr_core::resolve_quality(&parsed(axes.0, axes.1), &ranking);
        // No custom formats -> candidate CF score is 0, never above the on-disk
        // file's `disk_cf`. The on-disk file sits at the same quality.
        let formats: Vec<CustomFormat> = vec![];
        let prof = QualityProfile {
            id: QualityProfileId::new(),
            name: "p".to_string(),
            allowed_qualities: all_ranks(&ranking),
            upgrades_allowed: true,
            // Both cutoffs at/below the on-disk standing -> both met.
            cutoff_quality: q.rank,
            min_custom_format_score: i32::MIN,
            upgrade_until_custom_format_score: disk_cf,
            required_languages: vec![],
        };
        let ctx = DecisionContext {
            profile: &prof,
            custom_formats: &formats,
            ranking: &ranking,
            blocklisted: false,
            proper_repack_policy: ProperRepackPolicy::DoNotPrefer,
        };
        let existing = OnDiskFile {
            file_id: MediaFileId::new(),
            quality_rank: q.rank,
            custom_format_score: disk_cf,
            release_type: None,
        };
        let decision = decide(
            content_ref(),
            &release(&[]),
            &parsed(axes.0, axes.1),
            Some(existing),
            &ctx,
        )
        .unwrap();
        prop_assert!(
            matches!(
                decision.verdict,
                Verdict::Reject {
                    reason: cellarr_core::RejectReason::CutoffAlreadyMet
                }
            ),
            "both cutoffs met must reject as CutoffAlreadyMet, got {:?}",
            decision.verdict
        );
    }

    /// Total-function fuzz: arbitrary scene-name fragments fed through the full
    /// parse -> custom-format match -> decide path never panic. The parser and
    /// decision engine are reachable from untrusted indexer titles, so a panic
    /// would be a denial of service.
    #[test]
    fn parse_match_decide_never_panics_on_arbitrary_fragments(
        s in r"[\x00-\x7f]{0,80}",
    ) {
        let parsed = cellarr_parse::parse_title(&s);
        let mut rel = release(&["freeleech".to_string()]);
        rel.title = s.clone();

        // A handful of CFs spanning the condition kinds, incl. a title regex that
        // is itself derived from the fuzz input (a pathological but legal pattern
        // shape) — compilation may fail, which is a handled outcome, not a panic.
        let title_pat = format!(r"\b{}\b", regex_escape(&s));
        let formats = vec![
            flag_format("freeleech", "freeleech", 10),
            CustomFormat {
                id: CustomFormatId::new(),
                name: "fuzz-title".to_string(),
                conditions: vec![Condition {
                    kind: ConditionKind::ReleaseTitle { pattern: title_pat },
                    required: false,
                    negate: false,
                }],
                score: 5,
            },
        ];

        // Matching: building the context may error on a bad regex; that is fine.
        if let Ok(ctx) = MatchContext::new(&formats) {
            let _ = ctx.matches(&formats[0], &rel, &parsed);
            let _ = score(&rel, &parsed, &formats, &ctx);
        }

        let ranking = QualityRanking::default();
        let prof = QualityProfile {
            id: QualityProfileId::new(),
            name: "p".to_string(),
            allowed_qualities: all_ranks(&ranking),
            upgrades_allowed: true,
            cutoff_quality: 0,
            min_custom_format_score: i32::MIN,
            upgrade_until_custom_format_score: 0,
            required_languages: vec![],
        };
        let ctx = DecisionContext {
            profile: &prof,
            custom_formats: &formats,
            ranking: &ranking,
            blocklisted: false,
            proper_repack_policy: ProperRepackPolicy::Prefer,
        };
        // decide returns Result (InvalidRegex is an error, not a panic); both
        // arms are acceptable — the property is only that it does not panic.
        let _ = decide(content_ref(), &rel, &parsed, None, &ctx);
    }
}

/// Minimal regex metacharacter escaping so a fuzz fragment used inside a title
/// pattern does not turn into a syntactically broken regex *by construction*
/// every time (we still want a mix of valid and invalid patterns; this keeps the
/// majority valid so the match path is exercised, not just the compile-error path).
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\^$.|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
