//! TRaSH/Recyclarr custom-format import round-trips into equivalent decisions.
//!
//! The JSON below mirrors the shape of community TRaSH custom-format files
//! (re-curated clean-room, not copied): each format carries a `trash_id`, a
//! `name`, and `specifications` with `implementation` / `negate` / `required` /
//! `fields`. Recommended scores are supplied separately (as Recyclarr does),
//! keyed by `trash_id`. The test asserts the import yields formats that score and
//! decide as expected.

use std::collections::HashMap;

use cellarr_core::{
    ConditionKind, ContentId, ContentRef, Coordinates, IndexerId, LibraryId, MediaType,
    ParsedRelease, Protocol, QualityProfile, QualityProfileId, QualityRanking, Release, Resolution,
    Source, Verdict,
};
use cellarr_decide::{decide, import_trash_custom_formats, score, DecisionContext, MatchContext};

const TRASH_JSON: &str = r#"
[
  {
    "trash_id": "guard-cam",
    "name": "CAM/TS (guard)",
    "specifications": [
      {
        "name": "CAMRip",
        "implementation": "ReleaseTitleSpecification",
        "negate": false,
        "required": false,
        "fields": { "value": "(?i)\\b(CAM|CAMRip|HDCAM|TS|TELESYNC)\\b" }
      }
    ]
  },
  {
    "trash_id": "freeleech",
    "name": "Freeleech",
    "specifications": [
      {
        "name": "Freeleech",
        "implementation": "IndexerFlagSpecification",
        "negate": false,
        "required": false,
        "fields": { "value": "freeleech" }
      }
    ]
  },
  {
    "trash_id": "bluray-tier",
    "name": "Bluray Tier 01",
    "specifications": [
      {
        "name": "Bluray",
        "implementation": "SourceSpecification",
        "negate": false,
        "required": true,
        "fields": { "value": 7 }
      },
      {
        "name": "1080p",
        "implementation": "ResolutionSpecification",
        "negate": false,
        "required": true,
        "fields": { "value": 1080 }
      }
    ]
  }
]
"#;

fn scores() -> HashMap<String, i32> {
    HashMap::from([
        ("guard-cam".to_string(), -10000),
        ("freeleech".to_string(), 25),
        ("bluray-tier".to_string(), 100),
    ])
}

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

#[test]
fn import_maps_implementations_to_condition_kinds_and_assigns_scores() {
    let formats = import_trash_custom_formats(TRASH_JSON, &scores()).expect("import succeeds");
    assert_eq!(formats.len(), 3);

    let cam = &formats[0];
    assert_eq!(cam.name, "CAM/TS (guard)");
    assert_eq!(cam.score, -10000);
    assert!(matches!(
        cam.conditions[0].kind,
        ConditionKind::ReleaseTitle { .. }
    ));

    let freeleech = &formats[1];
    assert_eq!(freeleech.score, 25);
    assert!(matches!(
        freeleech.conditions[0].kind,
        ConditionKind::IndexerFlag { .. }
    ));

    let bluray = &formats[2];
    assert_eq!(bluray.score, 100);
    assert!(bluray.conditions[0].required);
    assert!(matches!(
        bluray.conditions[0].kind,
        ConditionKind::Source {
            source: Source::Bluray
        }
    ));
    assert!(matches!(
        bluray.conditions[1].kind,
        ConditionKind::Resolution {
            resolution: Resolution::R1080p
        }
    ));
}

#[test]
fn imported_formats_score_a_release_as_the_sum_of_matches() {
    let formats = import_trash_custom_formats(TRASH_JSON, &scores()).unwrap();
    let ctx = MatchContext::new(&formats).unwrap();

    // Bluray-1080p freeleech: bluray-tier (100) + freeleech (25) = 125.
    let rel = release("Movie.2024.1080p.BluRay.x264-GROUP", &["freeleech"]);
    let mut p = ParsedRelease::new(&rel.title);
    p.source = Some(Source::Bluray);
    p.resolution = Some(Resolution::R1080p);
    assert_eq!(score(&rel, &p, &formats, &ctx), 125);
}

#[test]
fn imported_cam_guard_drives_a_below_minimum_reject() {
    let formats = import_trash_custom_formats(TRASH_JSON, &scores()).unwrap();
    let ranking = QualityRanking::default();

    let profile = QualityProfile {
        id: QualityProfileId::new(),
        name: "p".to_string(),
        // CAM is rank 2 and Bluray-1080p rank 21 in the default ranking.
        allowed_qualities: vec![2, 21],
        upgrades_allowed: true,
        cutoff_quality: 27,
        min_custom_format_score: 0,
        upgrade_until_custom_format_score: 1000,
        required_languages: vec![],
    };

    let rel = release("Movie.2024.CAMRip.x264-GROUP", &[]);
    let mut p = ParsedRelease::new(&rel.title);
    p.source = Some(Source::Cam);
    p.resolution = Some(Resolution::R480p);

    let ctx = DecisionContext {
        profile: &profile,
        custom_formats: &formats,
        ranking: &ranking,
        blocklisted: false,
        proper_repack_policy: Default::default(),
    };

    let decision = decide(
        ContentRef::new(
            ContentId::new(),
            LibraryId::new(),
            MediaType::Movie,
            Coordinates::Movie,
        )
        .unwrap(),
        &rel,
        &p,
        None,
        &ctx,
    )
    .unwrap();

    assert!(
        matches!(
            decision.verdict,
            Verdict::Reject {
                reason: cellarr_core::RejectReason::BelowMinimumCustomFormatScore
            }
        ),
        "CAM guard should drive a below-minimum reject, got {:?}",
        decision.verdict
    );
}

#[test]
fn unscored_imported_format_defaults_to_zero() {
    // No score for "bluray-tier" -> it imports with score 0 but still matchable.
    let mut partial = scores();
    partial.remove("bluray-tier");
    let formats = import_trash_custom_formats(TRASH_JSON, &partial).unwrap();
    let bluray = formats.iter().find(|f| f.name == "Bluray Tier 01").unwrap();
    assert_eq!(bluray.score, 0);
}

#[test]
fn unknown_implementation_is_a_hard_error() {
    let json = r#"
    [
      {
        "trash_id": "weird",
        "name": "Weird",
        "specifications": [
          { "name": "x", "implementation": "ReleaseTypeSpecification", "fields": { "value": 1 } }
        ]
      }
    ]
    "#;
    let err = import_trash_custom_formats(json, &HashMap::new()).unwrap_err();
    assert!(
        matches!(
            err,
            cellarr_decide::DecideError::UnsupportedTrashSpec { .. }
        ),
        "expected UnsupportedTrashSpec, got {err:?}"
    );
}
