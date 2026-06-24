//! Targeted tests that pin precedence and matching boundaries a mutation run
//! found unguarded. Each test is written to fail if the named operator/condition
//! in the source is flipped, so a regression in the safety-critical decision and
//! custom-format algebra is caught.
//!
//! Survivors addressed (from `cargo mutants` on cellarr-decide):
//! - decide.rs full-season re-grab guard (the four AND conditions).
//! - decide.rs higher-quality upgrade gating (`upgrades_allowed && rank < cutoff`)
//!   and the `>= cutoff` CutoffAlreadyMet branch.
//! - decide.rs `is_proper_or_repack` raw-title `proper`/`repack` fallback (`||`).
//! - matching.rs `raw ^ negate` for ReleaseTitle, IndexerFlag, and Size.

use cellarr_core::{
    Condition, ConditionKind, ContentId, ContentRef, Coordinates, CustomFormat, CustomFormatId,
    IndexerId, LibraryId, MediaFileId, MediaType, ParsedRelease, ProperRepack, Protocol,
    QualityProfile, QualityProfileId, QualityRanking, Release, ReleaseType, Resolution, Source,
    Verdict,
};
use cellarr_decide::{decide, DecisionContext, MatchContext, OnDiskFile, ProperRepackPolicy};

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
        indexer_criteria: Default::default(),
        indexer_priority: 0,
    }
}

mod rank {
    pub const WEBDL_1080P: u32 = 20;
    pub const BLURAY_1080P: u32 = 21;
    pub const BLURAY_2160P: u32 = 26;
    pub const REMUX_2160P: u32 = 27;
}

// --- decide.rs: the full-season re-grab guard (lines 170-173) ---------------
//
// The guard fires only when ALL of: existing is FullSeason, candidate is
// FullSeason, candidate quality <= existing, candidate cf <= existing. When it
// fires it rejects (NotAnUpgrade). The single test below makes every one of the
// four conditions load-bearing: the standing is *equal* and the candidate is a
// PROPER, so if the guard is (wrongly) bypassed the equal-quality branch would
// PREFER the proper and return Upgrade. Flipping any of the four AND-conditions
// (`==`->`!=` or `<=`->`>`) bypasses the guard and yields Upgrade, so each flip
// is caught by the asserted Reject.

fn full_season_parsed(source: Source, resolution: Resolution, proper: bool) -> ParsedRelease {
    let mut p = parsed(source, resolution);
    p.coordinates.push(Coordinates::SeasonPack { season: 2 });
    if proper {
        p.proper_repack = Some(ProperRepack::Proper);
    }
    p
}

#[test]
fn full_season_guard_fires_only_when_all_four_conditions_hold() {
    let ranking = QualityRanking::default();
    let formats: Vec<CustomFormat> = vec![];
    // Cutoffs are far above the standing so neither cutoff is met (otherwise the
    // equal-quality branch would early-return CutoffAlreadyMet and mask the
    // proper-prefer Upgrade we rely on to detect a bypassed guard).
    let prof = profile(&[rank::WEBDL_1080P], rank::REMUX_2160P, 100_000);

    // Candidate: a PROPER full-season pack at the SAME standing as on disk.
    let rel = release("The.Show.S02.PROPER.1080p.WEB-DL-GROUP", &[]);
    let p = full_season_parsed(Source::WebDl, Resolution::R1080p, true);

    let existing = OnDiskFile {
        file_id: MediaFileId::new(),
        quality_rank: rank::WEBDL_1080P,
        custom_format_score: 0,
        release_type: Some(ReleaseType::FullSeason),
    };

    let decision = decide(
        content_ref(),
        &rel,
        &p,
        Some(existing),
        &ctx(&prof, &formats, &ranking),
    )
    .unwrap();

    // Guard must fire: an already-held identical full-season pack is rejected,
    // even though it is a PROPER (the guard out-ranks proper-prefer). If any of
    // the four guard conditions is flipped the guard is skipped and the PROPER is
    // preferred -> Upgrade, which this assertion rejects.
    match decision.verdict {
        Verdict::Reject {
            reason: cellarr_core::RejectReason::NotAnUpgrade,
        } => {}
        other => panic!("full-season guard must reject (NotAnUpgrade), got {other:?}"),
    }
}

#[test]
fn full_season_guard_does_not_suppress_a_non_full_season_proper_upgrade() {
    // The mirror that makes the `candidate_type == FullSeason` condition (line
    // 171) load-bearing in the OTHER direction: a single-episode PROPER at equal
    // standing against a FullSeason on-disk file must still Upgrade (the guard
    // must NOT fire, because the candidate is not a full season). If line 171's
    // `==` were `!=`, the guard would wrongly fire here and reject.
    let ranking = QualityRanking::default();
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile(&[rank::WEBDL_1080P], rank::REMUX_2160P, 100_000);

    let rel = release("The.Show.S02E05.PROPER.1080p.WEB-DL-GROUP", &[]);
    let mut p = parsed(Source::WebDl, Resolution::R1080p);
    p.coordinates.push(Coordinates::Episode {
        season: 2,
        episode: 5,
        absolute: None,
    });
    p.proper_repack = Some(ProperRepack::Proper);

    let existing = OnDiskFile {
        file_id: MediaFileId::new(),
        quality_rank: rank::WEBDL_1080P,
        custom_format_score: 0,
        release_type: Some(ReleaseType::FullSeason),
    };

    let decision = decide(
        content_ref(),
        &rel,
        &p,
        Some(existing),
        &ctx(&prof, &formats, &ranking),
    )
    .unwrap();

    assert!(
        matches!(decision.verdict, Verdict::Upgrade { .. }),
        "a single-episode PROPER must not be suppressed by the full-season guard, got {:?}",
        decision.verdict
    );
}

// --- decide.rs: higher-quality upgrade gating (line 183) --------------------

#[test]
fn higher_quality_with_upgrades_disabled_is_rejected_not_upgraded() {
    // Line 183: `ctx.profile.upgrades_allowed && existing.quality_rank < cutoff`.
    // A strictly higher-quality candidate must NOT upgrade when upgrades are
    // disabled. If the `&&` is mutated to `||` it would upgrade anyway.
    let ranking = QualityRanking::default();
    let formats: Vec<CustomFormat> = vec![];
    let mut prof = profile(
        &[rank::BLURAY_1080P, rank::BLURAY_2160P],
        rank::REMUX_2160P,
        100_000,
    );
    prof.upgrades_allowed = false;

    let rel = release("Movie.2024.2160p.BluRay-GROUP", &[]);
    let p = parsed(Source::Bluray, Resolution::R2160p);

    let decision = decide(
        content_ref(),
        &rel,
        &p,
        Some(OnDiskFile {
            file_id: MediaFileId::new(),
            quality_rank: rank::BLURAY_1080P,
            custom_format_score: 0,
            release_type: None,
        }),
        &ctx(&prof, &formats, &ranking),
    )
    .unwrap();

    match decision.verdict {
        Verdict::Reject {
            reason: cellarr_core::RejectReason::NotAnUpgrade,
        } => {}
        other => {
            panic!("higher quality with upgrades disabled must reject NotAnUpgrade, got {other:?}")
        }
    }
}

#[test]
fn higher_quality_candidate_when_existing_at_cutoff_is_cutoff_already_met() {
    // Lines 183 (`existing.quality_rank < cutoff` is false) and 190
    // (`existing.quality_rank >= cutoff` is true): a higher-quality candidate
    // when the on-disk file already sits AT the quality cutoff must reject with
    // CutoffAlreadyMet (not NotAnUpgrade, and not Upgrade). This pins both the
    // `<` boundary on 183 and the `>=` boundary on 190.
    let ranking = QualityRanking::default();
    let formats: Vec<CustomFormat> = vec![];
    // Cutoff == existing quality (Bluray-1080p): the existing file is AT cutoff.
    let prof = profile(
        &[rank::BLURAY_1080P, rank::BLURAY_2160P],
        rank::BLURAY_1080P,
        100_000,
    );

    let rel = release("Movie.2024.2160p.BluRay-GROUP", &[]);
    let p = parsed(Source::Bluray, Resolution::R2160p);

    let decision = decide(
        content_ref(),
        &rel,
        &p,
        Some(OnDiskFile {
            file_id: MediaFileId::new(),
            quality_rank: rank::BLURAY_1080P,
            custom_format_score: 0,
            release_type: None,
        }),
        &ctx(&prof, &formats, &ranking),
    )
    .unwrap();

    match decision.verdict {
        Verdict::Reject {
            reason: cellarr_core::RejectReason::CutoffAlreadyMet,
        } => {}
        other => panic!(
            "higher quality over an at-cutoff file must reject CutoffAlreadyMet, got {other:?}"
        ),
    }
}

#[test]
fn higher_quality_below_cutoff_with_upgrades_allowed_does_upgrade() {
    // The positive control: a strictly higher quality, upgrades allowed, existing
    // strictly below cutoff -> Upgrade. Together with the two rejects above this
    // makes line 183's `<` and line 190's `>=` boundaries load-bearing.
    let ranking = QualityRanking::default();
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile(
        &[rank::BLURAY_1080P, rank::BLURAY_2160P],
        rank::REMUX_2160P,
        100_000,
    );

    let rel = release("Movie.2024.2160p.BluRay-GROUP", &[]);
    let p = parsed(Source::Bluray, Resolution::R2160p);

    let decision = decide(
        content_ref(),
        &rel,
        &p,
        Some(OnDiskFile {
            file_id: MediaFileId::new(),
            quality_rank: rank::BLURAY_1080P,
            custom_format_score: 0,
            release_type: None,
        }),
        &ctx(&prof, &formats, &ranking),
    )
    .unwrap();

    assert!(
        matches!(decision.verdict, Verdict::Upgrade { .. }),
        "higher quality below cutoff with upgrades allowed must upgrade, got {:?}",
        decision.verdict
    );
}

// --- decide.rs: is_proper_or_repack raw-title fallback (line 276) ------------

#[test]
fn repack_recognized_from_raw_title_even_without_a_parsed_marker() {
    // Line 276: `lower.contains("proper") || lower.contains("repack")`. The parse
    // has NO proper_repack set; only the raw title says REPACK (not PROPER). If
    // the `||` were `&&`, the title would need BOTH words and this REPACK would be
    // missed, so an equal-standing REPACK would reject instead of upgrade.
    let ranking = QualityRanking::default();
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile(&[rank::BLURAY_1080P], rank::REMUX_2160P, 100_000);

    let rel = release("Movie.2024.REPACK.1080p.BluRay-GROUP", &[]);
    // Deliberately leave p.proper_repack = None so the raw-title fallback is the
    // only thing that can recognize this as a repack.
    let p = parsed(Source::Bluray, Resolution::R1080p);
    assert!(p.proper_repack.is_none());

    let decision = decide(
        content_ref(),
        &rel,
        &p,
        Some(OnDiskFile {
            file_id: MediaFileId::new(),
            quality_rank: rank::BLURAY_1080P,
            custom_format_score: 0,
            release_type: None,
        }),
        &ctx(&prof, &formats, &ranking),
    )
    .unwrap();

    assert!(
        matches!(decision.verdict, Verdict::Upgrade { .. }),
        "a REPACK recognized from the raw title must be preferred at equal standing, got {:?}",
        decision.verdict
    );
}

// --- matching.rs: `raw ^ negate` for the crate-evaluated conditions ---------
//
// For each condition kind the decision engine evaluates itself, `negate` must
// INVERT the raw match (`raw ^ negate`). A mutation to `raw | negate` makes a
// negated condition match unconditionally. Each test below uses a release where
// the raw fact is TRUE and `negate` is TRUE, so the correct result is FALSE
// (`true ^ true`) but the mutant `true | true` is TRUE.

fn cf(name: &str, kind: ConditionKind, negate: bool) -> CustomFormat {
    CustomFormat {
        id: CustomFormatId::new(),
        name: name.to_string(),
        conditions: vec![Condition {
            kind,
            required: false,
            negate,
        }],
        score: 0,
    }
}

fn rel_for_match(title: &str, flags: &[&str], size: Option<u64>) -> (Release, ParsedRelease) {
    let mut r = release(title, flags);
    r.size = size;
    (r, ParsedRelease::new(title))
}

#[test]
fn release_title_negate_inverts_a_raw_match() {
    // Line 176. Title contains "hevc"; condition is negated -> must NOT match.
    let fmt = cf(
        "not-hevc",
        ConditionKind::ReleaseTitle {
            pattern: "hevc".to_string(),
        },
        true,
    );
    let mc = MatchContext::new(std::slice::from_ref(&fmt)).expect("compiles");
    let (r, p) = rel_for_match("Show.S01E01.1080p.WEB-DL.HEVC-GRP", &[], None);
    assert!(
        !mc.matches(&fmt, &r, &p),
        "a negated ReleaseTitle whose pattern IS present must not match"
    );

    // Control: the same pattern non-negated DOES match, so the test isn't
    // vacuously true.
    let fmt_pos = cf(
        "hevc",
        ConditionKind::ReleaseTitle {
            pattern: "hevc".to_string(),
        },
        false,
    );
    let mc2 = MatchContext::new(std::slice::from_ref(&fmt_pos)).expect("compiles");
    assert!(mc2.matches(&fmt_pos, &r, &p));
}

#[test]
fn indexer_flag_negate_inverts_a_raw_match() {
    // Line 196. The release HAS the freeleech flag; negated -> must NOT match.
    let fmt = cf(
        "not-freeleech",
        ConditionKind::IndexerFlag {
            flag: "freeleech".to_string(),
        },
        true,
    );
    let mc = MatchContext::new(std::slice::from_ref(&fmt)).expect("compiles");
    let (r, p) = rel_for_match("Movie.2024.1080p-GRP", &["freeleech"], None);
    assert!(
        !mc.matches(&fmt, &r, &p),
        "a negated IndexerFlag whose flag IS present must not match"
    );

    let fmt_pos = cf(
        "freeleech",
        ConditionKind::IndexerFlag {
            flag: "freeleech".to_string(),
        },
        false,
    );
    let mc2 = MatchContext::new(std::slice::from_ref(&fmt_pos)).expect("compiles");
    assert!(mc2.matches(&fmt_pos, &r, &p));
}

#[test]
fn size_negate_inverts_a_raw_match() {
    // Line 202. The release size IS within [min,max]; negated -> must NOT match.
    let fmt = cf(
        "not-in-size-band",
        ConditionKind::Size {
            min: Some(1_000),
            max: Some(10_000),
        },
        true,
    );
    let mc = MatchContext::new(std::slice::from_ref(&fmt)).expect("compiles");
    let (r, p) = rel_for_match("Movie.2024.1080p-GRP", &[], Some(5_000));
    assert!(
        !mc.matches(&fmt, &r, &p),
        "a negated Size whose value IS in band must not match"
    );

    let fmt_pos = cf(
        "in-size-band",
        ConditionKind::Size {
            min: Some(1_000),
            max: Some(10_000),
        },
        false,
    );
    let mc2 = MatchContext::new(std::slice::from_ref(&fmt_pos)).expect("compiles");
    assert!(mc2.matches(&fmt_pos, &r, &p));
}
