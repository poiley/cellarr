//! Per-quality size-bounds gating in the decision engine.
//!
//! A release whose size-per-minute (reported size / content runtime) falls below
//! the quality's `min_size_per_min` or above its `max_size_per_min` is rejected
//! with [`RejectReason::QualitySizeOutOfBounds`], carrying the breached `bound`.
//! The gate **fails open**: an unknown release size or an unknown/zero content
//! runtime never produces a size rejection, so absent metadata can never cause a
//! false negative.

use cellarr_core::{
    ContentId, ContentRef, Coordinates, CustomFormat, IndexerCriteria, IndexerId, LibraryId,
    MediaType, ParsedRelease, Protocol, QualityProfile, QualityProfileId, QualityRanking,
    RejectReason, Release, Resolution, SizeBound, Source, Verdict,
};
use cellarr_decide::{decide, DecisionContext, ProperRepackPolicy};

/// A Bluray-1080p movie release with an optional reported `size` in bytes.
fn release(size: Option<u64>) -> Release {
    Release {
        indexer_id: IndexerId::new(),
        title: "Movie.2024.1080p.BluRay-GROUP".to_string(),
        download_url: "magnet:?xt=urn:test".to_string(),
        guid: None,
        protocol: Protocol::Usenet,
        size,
        seeders: None,
        indexer_flags: vec![],
    }
}

fn parsed() -> ParsedRelease {
    let mut p = ParsedRelease::new("Movie.2024.1080p.BluRay-GROUP");
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

/// The rank of the Bluray-1080p bucket in the default ranking.
fn bluray_1080p_rank() -> u32 {
    QualityRanking::default()
        .by_name("Bluray-1080p")
        .expect("present")
        .rank
}

fn profile() -> QualityProfile {
    QualityProfile {
        id: QualityProfileId::new(),
        name: "p".to_string(),
        allowed_qualities: vec![bluray_1080p_rank()],
        upgrades_allowed: true,
        cutoff_quality: 26,
        min_custom_format_score: 0,
        upgrade_until_custom_format_score: 100_000,
        required_languages: vec![],
    }
}

/// A default ranking with `[min, max]` size-per-minute bounds set on Bluray-1080p.
fn ranking_with_bounds(min: Option<u64>, max: Option<u64>) -> QualityRanking {
    let mut r = QualityRanking::default();
    let rank = bluray_1080p_rank();
    let def = r
        .qualities
        .iter_mut()
        .find(|q| q.rank == rank)
        .expect("present");
    def.min_size_per_min = min;
    def.max_size_per_min = max;
    r
}

fn ctx<'a>(
    profile: &'a QualityProfile,
    formats: &'a [CustomFormat],
    ranking: &'a QualityRanking,
    content_runtime: Option<u32>,
) -> DecisionContext<'a> {
    DecisionContext {
        profile,
        custom_formats: formats,
        ranking,
        blocklisted: false,
        proper_repack_policy: ProperRepackPolicy::Prefer,
        indexer_criteria: IndexerCriteria::default(),
        indexer_priority: 0,
        content_runtime,
    }
}

#[test]
fn a_release_above_the_maximum_size_is_rejected() {
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile();
    // Max 100 bytes/min; 100 min runtime; 20_000 bytes -> 200/min (> 100).
    let ranking = ranking_with_bounds(Some(1), Some(100));
    let rel = release(Some(20_000));
    let d = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &formats, &ranking, Some(100)),
    )
    .unwrap();
    assert_eq!(
        d.verdict,
        Verdict::Reject {
            reason: RejectReason::QualitySizeOutOfBounds {
                bound: SizeBound::AboveMaximum
            }
        },
        "got {:?}",
        d.verdict
    );
}

#[test]
fn a_release_below_the_minimum_size_is_rejected() {
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile();
    // Min 100 bytes/min; 100 min runtime; 1_000 bytes -> 10/min (< 100).
    let ranking = ranking_with_bounds(Some(100), None);
    let rel = release(Some(1_000));
    let d = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &formats, &ranking, Some(100)),
    )
    .unwrap();
    assert_eq!(
        d.verdict,
        Verdict::Reject {
            reason: RejectReason::QualitySizeOutOfBounds {
                bound: SizeBound::BelowMinimum
            }
        },
        "got {:?}",
        d.verdict
    );
}

#[test]
fn an_in_bounds_release_passes() {
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile();
    // [10, 200] bytes/min; 100 min; 5_000 bytes -> 50/min (in bounds) -> Grab.
    let ranking = ranking_with_bounds(Some(10), Some(200));
    let rel = release(Some(5_000));
    let d = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &formats, &ranking, Some(100)),
    )
    .unwrap();
    assert!(
        matches!(d.verdict, Verdict::Grab { .. }),
        "got {:?}",
        d.verdict
    );
}

#[test]
fn an_unknown_runtime_never_rejects_on_size() {
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile();
    // Bounds would reject this size, but runtime is unknown -> fail open -> Grab.
    let ranking = ranking_with_bounds(Some(1), Some(10));
    let rel = release(Some(1_000_000));
    let d = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &formats, &ranking, None),
    )
    .unwrap();
    assert!(
        matches!(d.verdict, Verdict::Grab { .. }),
        "got {:?}",
        d.verdict
    );
}

#[test]
fn an_unknown_size_never_rejects_on_size() {
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile();
    // Tight bounds, known runtime, but the release reports no size -> fail open.
    let ranking = ranking_with_bounds(Some(1_000), Some(2_000));
    let rel = release(None);
    let d = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &formats, &ranking, Some(100)),
    )
    .unwrap();
    assert!(
        matches!(d.verdict, Verdict::Grab { .. }),
        "got {:?}",
        d.verdict
    );
}

#[test]
fn raising_the_minimum_turns_a_previously_ok_release_into_a_reject() {
    // The "edit changes the decision" pin (mirrors the PUT-then-decide flow): the
    // same release that Grabs under loose bounds Rejects once the quality's minimum
    // is raised above its size-per-minute.
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile();
    let rel = release(Some(5_000)); // 100 min -> 50 bytes/min.

    // Before: min 10 -> in bounds -> Grab.
    let loose = ranking_with_bounds(Some(10), None);
    let before = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &formats, &loose, Some(100)),
    )
    .unwrap();
    assert!(
        matches!(before.verdict, Verdict::Grab { .. }),
        "got {:?}",
        before.verdict
    );

    // After raising the minimum to 100 (the PUT edit), the same release rejects.
    let tightened = ranking_with_bounds(Some(100), None);
    let after = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &formats, &tightened, Some(100)),
    )
    .unwrap();
    assert_eq!(
        after.verdict,
        Verdict::Reject {
            reason: RejectReason::QualitySizeOutOfBounds {
                bound: SizeBound::BelowMinimum
            }
        },
        "got {:?}",
        after.verdict
    );
}
