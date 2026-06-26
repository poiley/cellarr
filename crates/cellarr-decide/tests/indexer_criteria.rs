//! Per-indexer acceptance-criteria gating in the decision engine.
//!
//! Pins the three indexer-criteria rules added alongside the broadened indexer
//! config: a torrent below the indexer's `minimumSeeders` is rejected; a torrent
//! missing a required indexer flag (the freeleech-only policy) is rejected; and a
//! Usenet release is never gated by these torrent-only criteria. The seed
//! ratio/time criteria are exercised through their `RemovePolicy` projection.

use cellarr_core::{
    ContentId, ContentRef, Coordinates, CustomFormat, IndexerCriteria, IndexerId, LibraryId,
    MediaType, ParsedRelease, Protocol, QualityProfile, QualityProfileId, QualityRanking,
    RejectReason, Release, Resolution, Source, Verdict,
};
use cellarr_decide::{decide, DecisionContext, ProperRepackPolicy};

fn release(protocol: Protocol, seeders: Option<u32>, flags: &[&str]) -> Release {
    Release {
        indexer_id: IndexerId::new(),
        title: "Movie.2024.1080p.BluRay-GROUP".to_string(),
        download_url: "magnet:?xt=urn:test".to_string(),
        guid: None,
        protocol,
        size: None,
        seeders,
        indexer_flags: flags.iter().map(|s| s.to_string()).collect(),
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

fn profile() -> QualityProfile {
    // Allow the Bluray-1080p rank so a passing release would otherwise Grab.
    QualityProfile {
        id: QualityProfileId::new(),
        name: "p".to_string(),
        allowed_qualities: vec![21],
        upgrades_allowed: true,
        cutoff_quality: 26,
        min_custom_format_score: 0,
        upgrade_until_custom_format_score: 100_000,
        required_languages: vec![],
    }
}

fn ctx<'a>(
    profile: &'a QualityProfile,
    formats: &'a [CustomFormat],
    ranking: &'a QualityRanking,
    criteria: IndexerCriteria,
) -> DecisionContext<'a> {
    DecisionContext {
        profile,
        custom_formats: formats,
        ranking,
        blocklisted: false,
        proper_repack_policy: ProperRepackPolicy::Prefer,
        indexer_criteria: criteria,
        indexer_priority: 0,
        content_runtime: None,
        release_profiles: &[],
        content_tags: &[],
    }
}

#[test]
fn a_torrent_below_minimum_seeders_is_rejected_before_scoring() {
    let ranking = QualityRanking::default();
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile();
    let criteria = IndexerCriteria {
        minimum_seeders: Some(5),
        ..Default::default()
    };
    // 2 seeders < the floor of 5: rejected with the specific reason.
    let rel = release(Protocol::Torrent, Some(2), &[]);
    let d = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &formats, &ranking, criteria),
    )
    .unwrap();
    assert_eq!(
        d.verdict,
        Verdict::Reject {
            reason: RejectReason::InsufficientSeeders
        }
    );
}

#[test]
fn a_torrent_meeting_the_seeder_floor_is_not_rejected_for_seeders() {
    let ranking = QualityRanking::default();
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile();
    let criteria = IndexerCriteria {
        minimum_seeders: Some(5),
        ..Default::default()
    };
    // 5 >= 5: passes the seeder gate and (nothing on disk) Grabs.
    let rel = release(Protocol::Torrent, Some(5), &[]);
    let d = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &formats, &ranking, criteria),
    )
    .unwrap();
    assert!(
        matches!(d.verdict, Verdict::Grab { .. }),
        "got {:?}",
        d.verdict
    );
}

#[test]
fn a_torrent_reporting_no_seeders_fails_a_configured_floor() {
    let ranking = QualityRanking::default();
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile();
    let criteria = IndexerCriteria {
        minimum_seeders: Some(1),
        ..Default::default()
    };
    // An unreported seeder count cannot be proven to meet a floor.
    let rel = release(Protocol::Torrent, None, &[]);
    let d = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &formats, &ranking, criteria),
    )
    .unwrap();
    assert_eq!(
        d.verdict,
        Verdict::Reject {
            reason: RejectReason::InsufficientSeeders
        }
    );
}

#[test]
fn a_non_freeleech_torrent_is_rejected_when_the_indexer_requires_freeleech() {
    let ranking = QualityRanking::default();
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile();
    let criteria = IndexerCriteria {
        required_flags: vec!["freeleech".to_string()],
        ..Default::default()
    };
    // The release carries no flags -> missing the required freeleech flag.
    let rel = release(Protocol::Torrent, Some(50), &[]);
    let d = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &formats, &ranking, criteria),
    )
    .unwrap();
    assert_eq!(
        d.verdict,
        Verdict::Reject {
            reason: RejectReason::RequiredFlagMissing
        }
    );
}

#[test]
fn a_freeleech_torrent_passes_the_required_flag_gate_case_insensitively() {
    let ranking = QualityRanking::default();
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile();
    let criteria = IndexerCriteria {
        required_flags: vec!["freeleech".to_string()],
        ..Default::default()
    };
    // The release advertises FreeLeech (different case) -> the gate passes.
    let rel = release(Protocol::Torrent, Some(50), &["FreeLeech"]);
    let d = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &formats, &ranking, criteria),
    )
    .unwrap();
    assert!(
        matches!(d.verdict, Verdict::Grab { .. }),
        "got {:?}",
        d.verdict
    );
}

#[test]
fn usenet_releases_are_never_gated_by_torrent_only_criteria() {
    let ranking = QualityRanking::default();
    let formats: Vec<CustomFormat> = vec![];
    let prof = profile();
    // A criteria set that WOULD reject a torrent (seeder floor + freeleech) must be
    // a no-op for a Usenet release (no seeders / no freeleech concept).
    let criteria = IndexerCriteria {
        minimum_seeders: Some(100),
        required_flags: vec!["freeleech".to_string()],
        ..Default::default()
    };
    let rel = release(Protocol::Usenet, None, &[]);
    let d = decide(
        content_ref(),
        &rel,
        &parsed(),
        None,
        &ctx(&prof, &formats, &ranking, criteria),
    )
    .unwrap();
    assert!(
        matches!(d.verdict, Verdict::Grab { .. }),
        "got {:?}",
        d.verdict
    );
}

#[test]
fn seed_targets_project_minutes_to_seconds_for_the_remove_policy() {
    // The seed ratio/time criteria feed the download client's RemovePolicy; the
    // minutes->seconds projection is what the cleanup path consumes.
    let criteria = IndexerCriteria {
        seed_ratio: Some(2.0),
        seed_time_minutes: Some(1440), // 24h
        ..Default::default()
    };
    let (ratio, secs) = criteria.seed_targets_secs();
    assert_eq!(ratio, Some(2.0));
    assert_eq!(secs, Some(86_400));
}
