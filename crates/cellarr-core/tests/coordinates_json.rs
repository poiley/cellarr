//! Round-trip tests for the tagged-JSON form of [`Coordinates`] and the
//! media-type/coordinate validation invariants.

use cellarr_core::{ContentId, ContentRef, Coordinates, CoreError, LibraryId, MediaType};
use serde_json::json;

fn roundtrip(coords: Coordinates) {
    let value = serde_json::to_value(&coords).expect("serialize");
    let back: Coordinates = serde_json::from_value(value).expect("deserialize");
    assert_eq!(back, coords, "coordinates must round-trip exactly");
}

#[test]
fn movie_coordinates_round_trip() {
    roundtrip(Coordinates::Movie);
    let v = serde_json::to_value(Coordinates::Movie).unwrap();
    assert_eq!(v, json!({ "type": "movie" }));
}

#[test]
fn episode_coordinates_round_trip_with_and_without_absolute() {
    roundtrip(Coordinates::Episode {
        season: 3,
        episode: 7,
        absolute: None,
    });
    roundtrip(Coordinates::Episode {
        season: 0,
        episode: 0,
        absolute: Some(1071),
    });
    let v = serde_json::to_value(Coordinates::Episode {
        season: 2,
        episode: 15,
        absolute: None,
    })
    .unwrap();
    // `absolute: None` must be omitted from the tagged JSON.
    assert_eq!(v, json!({ "type": "episode", "season": 2, "episode": 15 }));
}

#[test]
fn track_coordinates_round_trip() {
    roundtrip(Coordinates::Track { disc: 1, track: 9 });
    let v = serde_json::to_value(Coordinates::Track { disc: 2, track: 3 }).unwrap();
    assert_eq!(v, json!({ "type": "track", "disc": 2, "track": 3 }));
}

#[test]
fn book_coordinates_round_trip_with_and_without_series_position() {
    roundtrip(Coordinates::Book {
        series_position: None,
    });
    roundtrip(Coordinates::Book {
        series_position: Some(4),
    });
    let v = serde_json::to_value(Coordinates::Book {
        series_position: None,
    })
    .unwrap();
    assert_eq!(v, json!({ "type": "book" }));
}

#[test]
fn coordinates_report_their_media_type() {
    assert_eq!(Coordinates::Movie.media_type(), MediaType::Movie);
    assert_eq!(
        Coordinates::Episode {
            season: 1,
            episode: 1,
            absolute: None
        }
        .media_type(),
        MediaType::Tv
    );
    assert_eq!(
        Coordinates::Track { disc: 1, track: 1 }.media_type(),
        MediaType::Music
    );
    assert_eq!(
        Coordinates::Book {
            series_position: None
        }
        .media_type(),
        MediaType::Book
    );
}

#[test]
fn content_ref_rejects_mismatched_coordinates() {
    let err = ContentRef::new(
        ContentId::new(),
        LibraryId::new(),
        MediaType::Movie,
        Coordinates::Track { disc: 1, track: 1 },
    )
    .expect_err("track coordinates in a movie library must be rejected");
    assert!(matches!(err, CoreError::InvalidCoordinates { .. }));
}

#[test]
fn content_ref_accepts_matching_coordinates() {
    let r = ContentRef::new(
        ContentId::new(),
        LibraryId::new(),
        MediaType::Tv,
        Coordinates::Episode {
            season: 1,
            episode: 2,
            absolute: None,
        },
    )
    .expect("matching coordinates must be accepted");
    assert_eq!(r.media_type, MediaType::Tv);
}
