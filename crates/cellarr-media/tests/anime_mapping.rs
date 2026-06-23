//! Anime absolute→season/episode remap correctness, driven by `corpus/anime/*`.
//!
//! These pin the swampiest correctness problem in the project (docs/02-data-model.md)
//! and the library-safety rule: an unmappable absolute number is surfaced for
//! manual resolution, never guessed.

mod common;

use cellarr_core::Coordinates;
use cellarr_media::identify::{remap_absolute, IdentifyError};
use cellarr_media::MediaError;

use common::{load_mapping_cases, load_unmapped_cases, provider_for};

#[tokio::test]
async fn corpus_absolute_maps_to_expected_season_episode() {
    let cases = load_mapping_cases();
    assert!(!cases.is_empty(), "corpus must supply mapping vectors");

    for case in &cases {
        let provider = provider_for(&case.external_id, &case.series, &case.mapping);
        let coords = Coordinates::Absolute {
            number: case.absolute,
        };

        let out = remap_absolute(&provider, &case.external_id, &coords)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "[{}] absolute {} should map, got error: {e}",
                    case.series, case.absolute
                )
            });

        match out {
            Coordinates::Episode {
                season,
                episode,
                absolute,
            } => {
                assert_eq!(
                    season, case.expected.season,
                    "[{}] absolute {} season",
                    case.series, case.absolute
                );
                assert_eq!(
                    episode, case.expected.episode,
                    "[{}] absolute {} episode",
                    case.series, case.absolute
                );
                // The absolute number is preserved through the remap so
                // downstream still knows the anime numbering.
                assert_eq!(
                    absolute,
                    Some(case.absolute),
                    "[{}] remap must preserve the absolute number",
                    case.series
                );
            }
            other => panic!("[{}] expected Episode, got {other:?}", case.series),
        }
    }
}

#[tokio::test]
async fn corpus_unmappable_absolute_is_surfaced_not_guessed() {
    let cases = load_unmapped_cases();
    assert!(!cases.is_empty(), "corpus must supply unmapped vectors");

    for case in &cases {
        let provider = provider_for(&case.external_id, &case.series, &case.mapping);
        let coords = Coordinates::Absolute {
            number: case.absolute,
        };

        let err = remap_absolute(&provider, &case.external_id, &coords)
            .await
            .expect_err(&format!(
                "[{}] absolute {} must NOT map (library safety)",
                case.series, case.absolute
            ));

        match (case.outcome.kind.as_str(), err) {
            ("unmapped", IdentifyError::Media(MediaError::UnmappedAbsolute { absolute, .. })) => {
                assert_eq!(absolute, case.absolute);
            }
            ("malformed", IdentifyError::Media(MediaError::MalformedSceneMapping { .. })) => {}
            (kind, other) => panic!(
                "[{}] expected outcome `{kind}`, got error {other:?}",
                case.series
            ),
        }
    }
}

#[tokio::test]
async fn absolute_with_no_mapping_at_all_is_unmapped() {
    // A series whose mapping the provider has never heard of must not be guessed.
    let provider = common::MockSceneProvider::default();
    let coords = Coordinates::Absolute { number: 5 };

    let err = remap_absolute(&provider, "tvdb-unknown", &coords)
        .await
        .expect_err("absence of a mapping must be an error, not a guess");
    assert!(matches!(
        err,
        IdentifyError::Media(MediaError::UnmappedAbsolute { .. })
    ));
}

#[tokio::test]
async fn non_absolute_coordinates_pass_through_unchanged() {
    let provider = common::MockSceneProvider::default();
    let already = Coordinates::Episode {
        season: 2,
        episode: 4,
        absolute: None,
    };
    let out = remap_absolute(&provider, "tvdb-anything", &already)
        .await
        .expect("non-absolute coords need no mapping");
    assert_eq!(out, already, "canonical coords must pass through untouched");
}
