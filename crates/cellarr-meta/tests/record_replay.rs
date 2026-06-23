//! Record/replay integration tests.
//!
//! These assert the source adapters normalize **recorded, synthetic** payloads
//! (documented shapes, see `tests/fixtures/README.md`) into the common schema,
//! and that the scene-mapping path remaps an absolute episode end to end. No
//! live source is touched: every adapter runs over a [`RecordedFetcher`].

use cellarr_core::{Coordinates, MediaType, MetadataSource};
use cellarr_meta::{
    parse_anime_list_entry, RecordedFetcher, TheTvdbConfig, TheTvdbSource, TmdbConfig, TmdbSource,
};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURES}/{name}")).expect("fixture present")
}

fn tmdb_with_key(fetcher: RecordedFetcher) -> TmdbSource<RecordedFetcher> {
    let config = TmdbConfig {
        api_key: Some("test-key".to_string()),
        ..TmdbConfig::default()
    };
    TmdbSource::new(fetcher, config)
}

fn tvdb_with_key(fetcher: RecordedFetcher) -> TheTvdbSource<RecordedFetcher> {
    let config = TheTvdbConfig {
        api_key: Some("test-key".to_string()),
        ..TheTvdbConfig::default()
    };
    TheTvdbSource::new(fetcher, config)
}

#[tokio::test]
async fn tmdb_search_normalizes_results() {
    let fetcher = RecordedFetcher::new().with_body(
        "https://api.themoviedb.org/3/search/movie",
        fixture("tmdb_search_movie.json"),
    );
    let source = tmdb_with_key(fetcher);
    let results = source.search_normalized("the matrix").await.unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].source_id, "603");
    assert_eq!(results[0].title, "The Matrix");
    assert_eq!(results[0].year, Some(1999));
    assert_eq!(results[0].media_type, MediaType::Movie);
    // The empty overview is normalized to None, not an empty string.
    assert_eq!(results[1].overview, None);
}

#[tokio::test]
async fn tmdb_fetch_normalizes_movie_with_ids_and_images() {
    let fetcher = RecordedFetcher::new().with_body(
        "https://api.themoviedb.org/3/movie/603",
        fixture("tmdb_movie.json"),
    );
    let source = tmdb_with_key(fetcher);
    let meta = source.fetch_normalized("603").await.unwrap();
    assert_eq!(meta.title, "The Matrix");
    assert_eq!(meta.year, Some(1999));
    assert!(meta
        .external_ids
        .contains(&("imdb".to_string(), "tt0133093".to_string())));
    assert!(meta
        .external_ids
        .contains(&("tmdb".to_string(), "603".to_string())));
    assert_eq!(meta.images.len(), 2);
    assert!(meta.images.iter().any(|i| i.kind == "poster"));
    assert!(meta.images.iter().any(|i| i.kind == "fanart"));
}

#[tokio::test]
async fn tmdb_without_key_reports_no_credential() {
    let source = TmdbSource::new(RecordedFetcher::new(), TmdbConfig::default());
    let err = source.search("anything").await.unwrap_err();
    assert!(matches!(
        err,
        cellarr_meta::MetaError::NoCredential { src: "tmdb" }
    ));
}

#[tokio::test]
async fn tmdb_trait_fetch_returns_json_value() {
    let fetcher = RecordedFetcher::new().with_body(
        "https://api.themoviedb.org/3/movie/603",
        fixture("tmdb_movie.json"),
    );
    let source = tmdb_with_key(fetcher);
    let value = source.fetch("603").await.unwrap();
    assert_eq!(value["title"], "The Matrix");
    assert_eq!(value["media_type"], "movie");
}

#[tokio::test]
async fn tvdb_search_normalizes_results() {
    let fetcher = RecordedFetcher::new().with_body(
        "https://api4.thetvdb.com/v4/search",
        fixture("tvdb_search_series.json"),
    );
    let source = tvdb_with_key(fetcher);
    let results = source.search_normalized("breaking bad").await.unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].source_id, "81189");
    assert_eq!(results[0].title, "Breaking Bad");
    assert_eq!(results[0].year, Some(2008));
    assert_eq!(results[0].media_type, MediaType::Tv);
}

#[tokio::test]
async fn tvdb_fetch_normalizes_series_with_child_structure() {
    let fetcher = RecordedFetcher::new().with_body(
        "https://api4.thetvdb.com/v4/series/81189/extended",
        fixture("tvdb_series_extended.json"),
    );
    let source = tvdb_with_key(fetcher);
    let meta = source.fetch_normalized("81189").await.unwrap();
    assert_eq!(meta.title, "Breaking Bad");
    assert_eq!(meta.year, Some(2008));
    assert!(meta
        .external_ids
        .contains(&("imdb".to_string(), "tt0903747".to_string())));
    // Child structure: three episodes across two seasons, absolute numbers kept.
    assert_eq!(meta.children.len(), 3);
    let s2 = meta
        .children
        .iter()
        .find(|c| c.season == Some(2))
        .expect("season 2 child");
    assert_eq!(s2.episode, Some(1));
    assert_eq!(s2.absolute, Some(8));
    assert_eq!(s2.title.as_deref(), Some("Seven Thirty-Seven"));
}

#[tokio::test]
async fn tvdb_scene_mapping_from_xem_fixture() {
    let fetcher = RecordedFetcher::new().with_body(
        "https://api4.thetvdb.com/v4/xem/map/all",
        fixture("xem_map_all.json"),
    );
    let source = tvdb_with_key(fetcher);
    let rules = source.scene_mapping("123").await.unwrap();
    assert!(!rules.is_empty());
    // The map remaps an absolute number to the right TVDB season/episode.
    let map = source.scene_map("123").await.unwrap();
    let remapped = map
        .remap_absolute(&Coordinates::Absolute { number: 3 })
        .unwrap();
    assert_eq!(
        remapped,
        Coordinates::Episode {
            season: 2,
            episode: 1,
            absolute: Some(3)
        }
    );
}

#[tokio::test]
async fn tvdb_scene_mapping_absent_is_empty_not_error() {
    // No XEM route registered → the recorder 404s → an empty (non-error) map.
    let source = tvdb_with_key(RecordedFetcher::new());
    let map = source.scene_map("999").await.unwrap();
    assert!(map.rules.is_empty());
    assert_eq!(map.tvdb_id.as_deref(), Some("999"));
}

#[tokio::test]
async fn anime_list_fixture_remaps_absolute_across_seasons() {
    let xml = String::from_utf8(fixture("anime_list_entry.xml")).unwrap();
    let map = parse_anime_list_entry(&xml).unwrap();
    // Absolute 13 is the first episode of TVDB season 2 in this two-cour anime.
    let remapped = map
        .remap_absolute(&Coordinates::Absolute { number: 13 })
        .unwrap();
    assert_eq!(
        remapped,
        Coordinates::Episode {
            season: 2,
            episode: 1,
            absolute: Some(13)
        }
    );
    // Absolute 7 stays in season 1 (the default-season fallback).
    let s1 = map
        .remap_absolute(&Coordinates::Absolute { number: 7 })
        .unwrap();
    assert_eq!(
        s1,
        Coordinates::Episode {
            season: 1,
            episode: 7,
            absolute: Some(7)
        }
    );
}

#[tokio::test]
async fn rate_limited_status_surfaces_as_http_error() {
    // A 429 from the source must surface as a typed Http error, not a decode.
    let fetcher = RecordedFetcher::new().with_response(
        "https://api.themoviedb.org/3/movie/603",
        429,
        b"{\"status_message\":\"rate limited\"}".to_vec(),
    );
    let source = tmdb_with_key(fetcher);
    let err = source.fetch_normalized("603").await.unwrap_err();
    assert!(matches!(
        err,
        cellarr_meta::MetaError::Http {
            src: "tmdb",
            status: 429
        }
    ));
}
