//! Dedicated tests for the precedence rules users most often get surprised by.
//!
//! The corpus (`tests/corpus.rs`) already covers these via vectors; these tests
//! pin the *exact* behavior in isolation, with prose names that read like the
//! spec, so a regression points straight at the violated rule.

use cellarr_core::{
    Condition, ConditionKind, ContentId, ContentRef, Coordinates, CustomFormat, CustomFormatId,
    IndexerId, LibraryId, MediaFileId, MediaType, ParsedRelease, ProperRepack, Protocol,
    QualityProfile, QualityProfileId, QualityRanking, Release, Resolution, Source, Verdict,
};
use cellarr_decide::{decide, DecisionContext, OnDiskFile, ProperRepackPolicy};

fn release(title: &str, flags: &[&str]) -> Release {
    Release {
        indexer_id: IndexerId::new(),
        title: title.to_string(),
        download_url: "magnet:?xt=urn:test".to_string(),
        guid: None,
        protocol: Protocol::Torrent,
        size: None,
        seeders: None,
        indexer_flags: flags.iter().map(|s| s.to_string()).collect(),
    }
}

fn parsed(source: Source, resolution: Resolution) -> ParsedRelease {
    let mut p = ParsedRelease::new("t");
    p.source = Some(source);
    p.resolution = Some(resolution);
    p
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

fn freeleech_format(score: i32) -> CustomFormat {
    CustomFormat {
        id: CustomFormatId::new(),
        name: "freeleech".to_string(),
        conditions: vec![Condition {
            kind: ConditionKind::IndexerFlag {
                flag: "freeleech".to_string(),
            },
            required: false,
            negate: false,
        }],
        score,
    }
}

fn profile(allowed: &[u32], cutoff_quality: u32, upgrade_until_cf: i32) -> QualityProfile {
    QualityProfile {
        id: QualityProfileId::new(),
        name: "p".to_string(),
        allowed_qualities: allowed.to_vec(),
        upgrades_allowed: true,
        cutoff_quality,
        min_custom_format_score: 0,
        upgrade_until_custom_format_score: upgrade_until_cf,
        required_languages: vec![],
    }
}

fn on_disk(quality_rank: u32, cf_score: i32) -> OnDiskFile {
    OnDiskFile {
        file_id: MediaFileId::new(),
        quality_rank,
        custom_format_score: cf_score,
    }
}

fn ctx<'a>(
    profile: &'a QualityProfile,
    formats: &'a [CustomFormat],
    ranking: &'a QualityRanking,
) -> DecisionContext<'a> {
    DecisionContext {
        profile,
        custom_formats: formats,
        ranking,
        blocklisted: false,
        proper_repack_policy: ProperRepackPolicy::Prefer,
    }
}

/// Ranks under the default ranking (cellarr_core::QualityRanking::default()),
/// for readability in the tests below.
mod rank {
    pub const WEBDL_1080P: u32 = 13;
    pub const BLURAY_1080P: u32 = 14;
    pub const BLURAY_2160P: u32 = 19;
}

#[test]
fn quality_rank_dominates_a_lower_quality_with_a_huge_cf_score_is_never_an_upgrade() {
    // Candidate: WEBDL-1080p (rank 13) carrying a +5000 CF score.
    // On disk:   Bluray-1080p (rank 14) with CF score 0.
    // The decision must NOT downgrade quality to chase the higher CF score.
    let ranking = QualityRanking::default();
    let formats = vec![freeleech_format(5000)];
    let prof = profile(
        &[rank::WEBDL_1080P, rank::BLURAY_1080P],
        rank::BLURAY_2160P,
        100_000,
    );
    let rel = release("Movie.2024.1080p.WEB-DL-GROUP", &["freeleech"]);
    let p = parsed(Source::WebDl, Resolution::R1080p);

    let decision = decide(
        content_ref(),
        &rel,
        &p,
        Some(on_disk(rank::BLURAY_1080P, 0)),
        &ctx(&prof, &formats, &ranking),
    )
    .unwrap();

    assert!(
        matches!(decision.verdict, Verdict::Reject { .. }),
        "lower quality with higher CF score must be rejected, got {:?}",
        decision.verdict
    );
}

#[test]
fn a_strictly_higher_quality_upgrades_even_when_its_cf_score_is_lower() {
    // The mirror of the rule above: higher quality wins regardless of CF score.
    let ranking = QualityRanking::default();
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile(
        &[rank::BLURAY_1080P, rank::BLURAY_2160P],
        20, // cutoff above 2160p so upgrades are allowed
        100_000,
    );
    let rel = release("Movie.2024.2160p.BluRay-GROUP", &[]);
    let p = parsed(Source::Bluray, Resolution::R2160p);

    let decision = decide(
        content_ref(),
        &rel,
        &p,
        Some(on_disk(rank::BLURAY_1080P, 500)), // existing has a high CF score
        &ctx(&prof, &formats, &ranking),
    )
    .unwrap();

    assert!(
        matches!(decision.verdict, Verdict::Upgrade { .. }),
        "higher quality must upgrade despite lower CF score, got {:?}",
        decision.verdict
    );
}

#[test]
fn both_cutoffs_met_stops_all_churn_even_for_a_higher_cf_candidate() {
    // On disk is at the quality cutoff AND at/above the CF-score cutoff.
    // A candidate at the same quality with an even higher CF score must be
    // rejected with CutoffAlreadyMet — no churn.
    let ranking = QualityRanking::default();
    let formats = vec![freeleech_format(1000)];
    let prof = profile(&[rank::BLURAY_1080P], rank::BLURAY_1080P, 100);
    let rel = release("Movie.2024.1080p.BluRay-GROUP", &["freeleech"]);
    let p = parsed(Source::Bluray, Resolution::R1080p);

    let decision = decide(
        content_ref(),
        &rel,
        &p,
        Some(on_disk(rank::BLURAY_1080P, 100)), // CF score == upgrade-until
        &ctx(&prof, &formats, &ranking),
    )
    .unwrap();

    match decision.verdict {
        Verdict::Reject {
            reason: cellarr_core::RejectReason::CutoffAlreadyMet,
        } => {}
        other => panic!("expected CutoffAlreadyMet, got {other:?}"),
    }
}

#[test]
fn only_one_cutoff_met_still_allows_an_upgrade_on_the_unmet_axis() {
    // Quality cutoff met, but CF-score cutoff unmet and the candidate's CF score
    // is higher -> still an upgrade. This is the contrapositive of "both to stop".
    let ranking = QualityRanking::default();
    let formats = vec![freeleech_format(150)];
    let prof = profile(&[rank::BLURAY_1080P], rank::BLURAY_1080P, 200);
    let rel = release("Movie.2024.1080p.BluRay-GROUP", &["freeleech"]);
    let p = parsed(Source::Bluray, Resolution::R1080p);

    let decision = decide(
        content_ref(),
        &rel,
        &p,
        Some(on_disk(rank::BLURAY_1080P, 0)),
        &ctx(&prof, &formats, &ranking),
    )
    .unwrap();

    assert!(
        matches!(decision.verdict, Verdict::Upgrade { .. }),
        "CF-score cutoff unmet should still upgrade, got {:?}",
        decision.verdict
    );
}

#[test]
fn hard_negative_guard_rejects_below_minimum_cf_score_before_any_upgrade_logic() {
    // A matching -10000 guard sinks the total below the profile minimum, so the
    // candidate is rejected as a hard reject even though it would otherwise be a
    // clean fresh grab with no file on disk.
    let ranking = QualityRanking::default();
    let guard = CustomFormat {
        id: CustomFormatId::new(),
        name: "x265-guard".to_string(),
        conditions: vec![Condition {
            kind: ConditionKind::Codec {
                codec: cellarr_core::VideoCodec::X265,
            },
            required: false,
            negate: false,
        }],
        score: -10000,
    };
    let mut prof = profile(&[rank::BLURAY_2160P], 20, 100);
    prof.min_custom_format_score = 0;
    let rel = release("Movie.2024.2160p.BluRay.x265-GROUP", &[]);
    let mut p = parsed(Source::Bluray, Resolution::R2160p);
    p.codec = Some(cellarr_core::VideoCodec::X265);

    let decision = decide(
        content_ref(),
        &rel,
        &p,
        None, // nothing on disk: a fresh grab, were it not for the guard
        &ctx(&prof, &[guard], &ranking),
    )
    .unwrap();

    match decision.verdict {
        Verdict::Reject {
            reason: cellarr_core::RejectReason::BelowMinimumCustomFormatScore,
        } => {}
        other => panic!("expected BelowMinimumCustomFormatScore, got {other:?}"),
    }
}

#[test]
fn proper_at_equal_quality_and_cf_is_preferred_only_under_the_prefer_policy() {
    let ranking = QualityRanking::default();
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile(&[rank::BLURAY_1080P], 20, 100);
    let rel = release("Movie.2024.PROPER.1080p.BluRay-GROUP", &[]);
    let mut p = parsed(Source::Bluray, Resolution::R1080p);
    p.proper_repack = Some(ProperRepack::Proper);

    let prefer = decide(
        content_ref(),
        &rel,
        &p,
        Some(on_disk(rank::BLURAY_1080P, 0)),
        &DecisionContext {
            profile: &prof,
            custom_formats: &formats,
            ranking: &ranking,
            blocklisted: false,
            proper_repack_policy: ProperRepackPolicy::Prefer,
        },
    )
    .unwrap();
    assert!(
        matches!(prefer.verdict, Verdict::Upgrade { .. }),
        "Prefer policy should upgrade a PROPER at equal standing, got {:?}",
        prefer.verdict
    );

    let do_not = decide(
        content_ref(),
        &rel,
        &p,
        Some(on_disk(rank::BLURAY_1080P, 0)),
        &DecisionContext {
            profile: &prof,
            custom_formats: &formats,
            ranking: &ranking,
            blocklisted: false,
            proper_repack_policy: ProperRepackPolicy::DoNotPrefer,
        },
    )
    .unwrap();
    assert!(
        matches!(do_not.verdict, Verdict::Reject { .. }),
        "DoNotPrefer policy should reject a PROPER at equal standing, got {:?}",
        do_not.verdict
    );
}
