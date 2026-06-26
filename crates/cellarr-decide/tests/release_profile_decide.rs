//! Release-profile gating + scoring through the decision engine.
//!
//! These pin the wiring of [`cellarr_core::ReleaseProfile`] into [`decide`]:
//! an ignored term rejects, a missing required term rejects (present passes), a
//! preferred term shifts the score (a positive term outranks a release without
//! it, a negative term demotes), and a tag-scoped profile applies only to tagged
//! content.

use cellarr_core::{
    ContentId, ContentRef, Coordinates, IndexerId, LibraryId, MediaType, ParsedRelease,
    PreferredTerm, Protocol, QualityProfile, QualityProfileId, QualityRanking, RejectReason,
    Release, ReleaseProfile, Resolution, Score, Source, Verdict,
};
use cellarr_decide::{decide, DecisionContext, ProperRepackPolicy};

fn release(title: &str) -> Release {
    Release {
        indexer_id: IndexerId::new(),
        title: title.to_string(),
        download_url: "magnet:?xt=urn:test".to_string(),
        guid: None,
        protocol: Protocol::Torrent,
        size: None,
        seeders: None,
        indexer_flags: vec![],
    }
}

fn parsed() -> ParsedRelease {
    let mut p = ParsedRelease::new("t");
    p.source = Some(Source::Bluray);
    p.resolution = Some(Resolution::R1080p);
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

/// A permissive profile so only the release profile (not quality/CF gating)
/// decides the verdict. `min_custom_format_score` is very negative so a demoting
/// preferred term never trips the min-score gate.
fn quality_profile() -> QualityProfile {
    QualityProfile {
        id: QualityProfileId::new(),
        // Bluray-1080p is rank 21 in the default ranking.
        allowed_qualities: vec![21],
        name: "p".to_string(),
        upgrades_allowed: true,
        cutoff_quality: 27,
        min_custom_format_score: -1_000_000,
        upgrade_until_custom_format_score: 1_000_000,
        required_languages: vec![],
    }
}

fn ctx<'a>(
    profile: &'a QualityProfile,
    ranking: &'a QualityRanking,
    release_profiles: &'a [ReleaseProfile],
    content_tags: &'a [u32],
) -> DecisionContext<'a> {
    DecisionContext {
        profile,
        custom_formats: &[],
        ranking,
        blocklisted: false,
        proper_repack_policy: ProperRepackPolicy::Prefer,
        indexer_criteria: Default::default(),
        indexer_priority: 0,
        content_runtime: None,
        release_profiles,
        content_tags,
    }
}

fn grab_score(verdict: &Verdict) -> Score {
    match verdict {
        Verdict::Grab { score } => *score,
        other => panic!("expected Grab, got {other:?}"),
    }
}

#[test]
fn ignored_term_in_title_rejects() {
    let ranking = QualityRanking::default();
    let prof = quality_profile();
    let mut rp = ReleaseProfile::new("no-x265");
    rp.ignored = vec!["x265".into()];
    let rps = vec![rp];

    let rel = release("Movie.2024.1080p.BluRay.x265-GROUP");
    let decision = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &ranking, &rps, &[]),
    )
    .unwrap();
    assert!(
        matches!(
            decision.verdict,
            Verdict::Reject {
                reason: RejectReason::ReleaseProfileIgnoredTerm { ref term }
            } if term == "x265"
        ),
        "an ignored term must reject, got {:?}",
        decision.verdict
    );
}

#[test]
fn required_term_absent_rejects_present_passes() {
    let ranking = QualityRanking::default();
    let prof = quality_profile();
    let mut rp = ReleaseProfile::new("must-bluray");
    rp.required = vec!["bluray".into()];
    let rps = vec![rp];

    // Absent -> reject.
    let webdl = release("Movie.2024.1080p.WEB-DL-GROUP");
    let rejected = decide(
        content_ref(),
        &webdl,
        &parsed(),
        None,
        &ctx(&prof, &ranking, &rps, &[]),
    )
    .unwrap();
    assert!(
        matches!(
            rejected.verdict,
            Verdict::Reject {
                reason: RejectReason::ReleaseProfileRequiredTermMissing
            }
        ),
        "a missing required term must reject, got {:?}",
        rejected.verdict
    );

    // Present -> passes (grab).
    let bluray = release("Movie.2024.1080p.BluRay-GROUP");
    let grabbed = decide(
        content_ref(),
        &bluray,
        &parsed(),
        None,
        &ctx(&prof, &ranking, &rps, &[]),
    )
    .unwrap();
    assert!(
        matches!(grabbed.verdict, Verdict::Grab { .. }),
        "a present required term must pass, got {:?}",
        grabbed.verdict
    );
}

#[test]
fn preferred_term_shifts_score_and_ranking() {
    let ranking = QualityRanking::default();
    let prof = quality_profile();

    // A profile that prefers "remux" (+100) and demotes "cam" (-100).
    let mut rp = ReleaseProfile::new("prefs");
    rp.preferred = vec![
        PreferredTerm {
            term: "remux".into(),
            score: 100,
        },
        PreferredTerm {
            term: "cam".into(),
            score: -100,
        },
    ];
    let rps = vec![rp];

    let plain = release("Movie.2024.1080p.BluRay-GROUP");
    let preferred = release("Movie.2024.1080p.BluRay.Remux-GROUP");
    let demoted = release("Movie.2024.1080p.BluRay.CAM-GROUP");

    let plain_score = grab_score(
        &decide(
            content_ref(),
            &plain,
            &parsed(),
            None,
            &ctx(&prof, &ranking, &rps, &[]),
        )
        .unwrap()
        .verdict,
    );
    let preferred_score = grab_score(
        &decide(
            content_ref(),
            &preferred,
            &parsed(),
            None,
            &ctx(&prof, &ranking, &rps, &[]),
        )
        .unwrap()
        .verdict,
    );
    let demoted_score = grab_score(
        &decide(
            content_ref(),
            &demoted,
            &parsed(),
            None,
            &ctx(&prof, &ranking, &rps, &[]),
        )
        .unwrap()
        .verdict,
    );

    // The preferred release outranks the plain one by exactly its preferred score;
    // the demoted one ranks below.
    assert_eq!(
        preferred_score.custom_format_score,
        plain_score.custom_format_score + 100
    );
    assert_eq!(
        demoted_score.custom_format_score,
        plain_score.custom_format_score - 100
    );
    assert!(preferred_score.custom_format_score > plain_score.custom_format_score);
    assert!(demoted_score.custom_format_score < plain_score.custom_format_score);
}

#[test]
fn tag_scoped_profile_applies_only_to_tagged_content() {
    let ranking = QualityRanking::default();
    let prof = quality_profile();

    // A profile scoped to tag id 7 that ignores "x265".
    let mut rp = ReleaseProfile::new("anime-no-x265");
    rp.tags = vec![7];
    rp.ignored = vec!["x265".into()];
    let rps = vec![rp];

    let rel = release("Movie.2024.1080p.BluRay.x265-GROUP");

    // Untagged content: the tagged profile does NOT apply -> grab.
    let untagged = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &ranking, &rps, &[]),
    )
    .unwrap();
    assert!(
        matches!(untagged.verdict, Verdict::Grab { .. }),
        "a tag-scoped profile must not gate untagged content, got {:?}",
        untagged.verdict
    );

    // Content carrying tag 7: the profile applies -> reject on the ignored term.
    let tagged = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &ranking, &rps, &[7]),
    )
    .unwrap();
    assert!(
        matches!(
            tagged.verdict,
            Verdict::Reject {
                reason: RejectReason::ReleaseProfileIgnoredTerm { .. }
            }
        ),
        "a tag-scoped profile must gate matching-tagged content, got {:?}",
        tagged.verdict
    );
}

#[test]
fn disabled_profile_gates_nothing() {
    let ranking = QualityRanking::default();
    let prof = quality_profile();
    let mut rp = ReleaseProfile::new("disabled");
    rp.enabled = false;
    rp.ignored = vec!["bluray".into()];
    let rps = vec![rp];

    let rel = release("Movie.2024.1080p.BluRay-GROUP");
    let decision = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &ranking, &rps, &[]),
    )
    .unwrap();
    assert!(
        matches!(decision.verdict, Verdict::Grab { .. }),
        "a disabled profile must gate nothing, got {:?}",
        decision.verdict
    );
}
