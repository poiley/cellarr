//! Scene-mapping correctness against the shared anime corpus.
//!
//! `corpus/anime/*` is shared with the parser and `cellarr-media`: the parser
//! produces the absolute number, and Identify (via this crate's scene mapping)
//! lands it on a TVDB season/episode. These tests assert this crate's
//! [`SceneMap::remap_absolute`] reproduces every corpus expectation, including
//! the library-safety cases that must be **surfaced**, never force-fit:
//! `unmapped` (release ahead of the mapping) and `malformed` (overlapping
//! ranges).
//!
//! The corpus expresses each mapping as `{ season, start_absolute, length }`
//! ranges; [`SceneMap::from_ranges`] consumes that shape directly.

use cellarr_core::Coordinates;
use cellarr_meta::{MetaError, SceneMap};
use serde::Deserialize;

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/anime");

#[derive(Debug, Deserialize)]
struct Range {
    season: u32,
    start_absolute: u32,
    length: u32,
}

#[derive(Debug, Deserialize)]
struct Mapping {
    ranges: Vec<Range>,
}

#[derive(Debug, Deserialize)]
struct Expected {
    season: u32,
    episode: u32,
}

#[derive(Debug, Deserialize)]
struct Outcome {
    kind: String,
}

#[derive(Debug, Deserialize)]
struct MappedCase {
    external_id: String,
    absolute: u32,
    mapping: Mapping,
    expected: Expected,
}

#[derive(Debug, Deserialize)]
struct MappedFile {
    case: Vec<MappedCase>,
}

#[derive(Debug, Deserialize)]
struct UnmappedCase {
    external_id: String,
    absolute: u32,
    mapping: Mapping,
    outcome: Outcome,
}

#[derive(Debug, Deserialize)]
struct UnmappedFile {
    case: Vec<UnmappedCase>,
}

fn scene_map(external_id: &str, mapping: &Mapping) -> SceneMap {
    let ranges: Vec<(u32, u32, u32)> = mapping
        .ranges
        .iter()
        .map(|r| (r.season, r.start_absolute, r.length))
        .collect();
    SceneMap::from_ranges(Some(external_id.to_string()), &ranges)
}

#[test]
fn corpus_absolute_to_season_episode_all_pass() {
    let raw = std::fs::read_to_string(format!("{CORPUS}/absolute_to_season_episode.toml"))
        .expect("corpus file present");
    let file: MappedFile = toml::from_str(&raw).expect("corpus parses");
    assert!(!file.case.is_empty(), "corpus must have cases");

    for case in &file.case {
        let map = scene_map(&case.external_id, &case.mapping);
        let got = map
            .remap_absolute(&Coordinates::Absolute {
                number: case.absolute,
            })
            .unwrap_or_else(|e| panic!("{} abs {}: {e}", case.external_id, case.absolute));
        assert_eq!(
            got,
            Coordinates::Episode {
                season: case.expected.season,
                episode: case.expected.episode,
                absolute: Some(case.absolute),
            },
            "{} abs {} mismatch",
            case.external_id,
            case.absolute
        );
    }
}

#[test]
fn corpus_unmapped_and_malformed_are_surfaced() {
    let raw = std::fs::read_to_string(format!("{CORPUS}/unmapped_absolute.toml"))
        .expect("corpus file present");
    let file: UnmappedFile = toml::from_str(&raw).expect("corpus parses");
    assert!(!file.case.is_empty(), "corpus must have cases");

    for case in &file.case {
        let map = scene_map(&case.external_id, &case.mapping);
        let err = map
            .remap_absolute(&Coordinates::Absolute {
                number: case.absolute,
            })
            .expect_err("must not place an unmappable/malformed absolute number");
        let MetaError::Unmappable { detail, .. } = err else {
            panic!("{}: expected Unmappable, got {err:?}", case.external_id);
        };
        // The corpus distinguishes the two failure modes; our detail string
        // names which one so callers can tell "ahead of mapping" from "data bug".
        assert!(
            detail.contains(&case.outcome.kind),
            "{} expected outcome '{}', detail was '{}'",
            case.external_id,
            case.outcome.kind,
            detail
        );
    }
}
