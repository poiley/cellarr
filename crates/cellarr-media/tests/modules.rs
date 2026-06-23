//! Movie and TV module behavior: search_terms, match_release (with confidence),
//! and naming_tokens, against in-memory fixtures.

mod common;

use std::collections::HashMap;

use cellarr_core::{
    Confidence, ContentId, Coordinates, LibraryId, MediaModule, MediaType, ParsedRelease,
};
use cellarr_media::content::ContentCandidate;
use cellarr_media::module::AMBIGUOUS_CONFIDENCE;
use cellarr_media::{MediaError, ModuleError, MovieModule, SeriesMeta, TvModule};

use common::{episode_ref, movie_ref, MockContentLookup, MockMetadata, MovieMeta};

// --- Movie module ----------------------------------------------------------

fn movie_meta(title: &str, year: Option<u16>, aliases: &[&str]) -> MovieMeta {
    MovieMeta {
        title: title.to_string(),
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
        year,
        external_ids: vec![("imdbid".to_string(), "tt0133093".to_string())],
    }
}

#[tokio::test]
async fn movie_search_terms_include_title_year_aliases_and_ids() {
    let lib = LibraryId::new();
    let node = movie_ref(lib);
    let mut meta = MockMetadata::default();
    meta.movies
        .insert(node.id, movie_meta("The Matrix", Some(1999), &["Matrix"]));
    let module = MovieModule::new(MockContentLookup { candidates: vec![] }, meta);

    let terms = module.search_terms(&node).await.expect("search terms");
    // Title+year first (most specific), then bare title, then aliases.
    assert_eq!(terms.queries[0], "The Matrix 1999");
    assert_eq!(terms.queries[1], "The Matrix");
    assert!(terms.queries.contains(&"Matrix".to_string()));
    assert!(terms
        .ids
        .contains(&("imdbid".to_string(), "tt0133093".to_string())));
    assert!(terms.numbering.is_empty(), "movies carry no numbering");
}

#[tokio::test]
async fn movie_match_exact_title_is_certain() {
    let lib = LibraryId::new();
    let node = movie_ref(lib);
    let candidates = vec![ContentCandidate {
        content_ref: node.clone(),
        title: "The Matrix".to_string(),
        aliases: vec![],
    }];
    let module = MovieModule::new(MockContentLookup { candidates }, MockMetadata::default());

    let mut parsed = ParsedRelease::new("The.Matrix.1999.1080p.BluRay.x264-GROUP");
    parsed.clean_title = Some("The Matrix".to_string());

    let matches = module.match_release(&parsed).await.expect("match");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].confidence, Confidence::CERTAIN);
    assert_eq!(matches[0].content_ref.id, node.id);
}

#[tokio::test]
async fn movie_match_via_alias_is_high_not_certain() {
    let lib = LibraryId::new();
    let node = movie_ref(lib);
    let candidates = vec![ContentCandidate {
        content_ref: node.clone(),
        title: "The Matrix".to_string(),
        aliases: vec!["Matrix".to_string()],
    }];
    let module = MovieModule::new(MockContentLookup { candidates }, MockMetadata::default());

    let mut parsed = ParsedRelease::new("Matrix.1999.1080p");
    parsed.clean_title = Some("Matrix".to_string());

    let matches = module.match_release(&parsed).await.expect("match");
    assert_eq!(matches.len(), 1);
    let c = matches[0].confidence.value();
    assert!(
        c < 1.0 && c > AMBIGUOUS_CONFIDENCE,
        "alias match is high: {c}"
    );
}

#[tokio::test]
async fn movie_ambiguous_two_same_titles_demoted_to_manual() {
    // Two distinct movie nodes that both carry the parse's title (e.g. a remake
    // and the original both titled "The Thing"): the title is ambiguous, so each
    // match is demoted so the caller routes it to manual resolution.
    let lib = LibraryId::new();
    let a = movie_ref(lib);
    let b = movie_ref(lib);
    let candidates = vec![
        ContentCandidate {
            content_ref: a,
            title: "The Thing".to_string(),
            aliases: vec![],
        },
        ContentCandidate {
            content_ref: b,
            title: "The Thing".to_string(),
            aliases: vec![],
        },
    ];
    let module = MovieModule::new(MockContentLookup { candidates }, MockMetadata::default());

    let mut parsed = ParsedRelease::new("The.Thing.1080p");
    parsed.clean_title = Some("The Thing".to_string());

    let matches = module.match_release(&parsed).await.expect("match");
    assert_eq!(matches.len(), 2, "both surfaced, not force-fit to one");
    for m in &matches {
        assert!(
            m.confidence.value() <= AMBIGUOUS_CONFIDENCE,
            "ambiguous matches must be demoted: {}",
            m.confidence.value()
        );
    }
}

#[tokio::test]
async fn movie_no_title_match_yields_no_matches() {
    let lib = LibraryId::new();
    let candidates = vec![ContentCandidate {
        content_ref: movie_ref(lib),
        title: "Some Other Film".to_string(),
        aliases: vec![],
    }];
    let module = MovieModule::new(MockContentLookup { candidates }, MockMetadata::default());

    let mut parsed = ParsedRelease::new("The.Matrix.1999");
    parsed.clean_title = Some("The Matrix".to_string());

    let matches = module.match_release(&parsed).await.expect("match");
    assert!(matches.is_empty());
}

#[tokio::test]
async fn movie_naming_tokens_have_title_and_year() {
    let lib = LibraryId::new();
    let node = movie_ref(lib);
    let mut meta = MockMetadata::default();
    meta.movies
        .insert(node.id, movie_meta("The Matrix", Some(1999), &[]));
    let module = MovieModule::new(MockContentLookup { candidates: vec![] }, meta);

    let tokens = module.naming_tokens(&node).await.expect("tokens").tokens;
    assert!(tokens.contains(&("Movie Title".to_string(), "The Matrix".to_string())));
    assert!(tokens.contains(&("Release Year".to_string(), "1999".to_string())));
}

#[tokio::test]
async fn movie_unresolved_identity_is_an_error() {
    let lib = LibraryId::new();
    let node = movie_ref(lib);
    // No metadata registered for the node.
    let module = MovieModule::new(
        MockContentLookup { candidates: vec![] },
        MockMetadata::default(),
    );
    let err = module.search_terms(&node).await.expect_err("no identity");
    assert!(matches!(
        err,
        ModuleError::Media(MediaError::UnresolvedIdentity(_))
    ));
}

#[tokio::test]
async fn movie_module_rejects_tv_node() {
    let lib = LibraryId::new();
    let tv = episode_ref(lib, 1, 1);
    let module = MovieModule::new(
        MockContentLookup { candidates: vec![] },
        MockMetadata::default(),
    );
    let err = module.search_terms(&tv).await.expect_err("wrong type");
    assert!(matches!(
        err,
        ModuleError::Media(MediaError::WrongMediaType { .. })
    ));
}

// --- TV module -------------------------------------------------------------

fn series_meta(title: &str, aliases: &[&str]) -> SeriesMeta {
    SeriesMeta {
        title: title.to_string(),
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
        year: Some(2008),
        external_ids: vec![("tvdbid".to_string(), "81189".to_string())],
    }
}

#[tokio::test]
async fn tv_search_terms_include_season_and_episode_numbering() {
    let lib = LibraryId::new();
    let node = episode_ref(lib, 2, 5);
    let mut meta = MockMetadata::default();
    meta.series
        .insert(node.id, series_meta("Breaking Bad", &[]));
    let module = TvModule::new(MockContentLookup { candidates: vec![] }, meta);

    let terms = module.search_terms(&node).await.expect("terms");
    assert_eq!(terms.queries[0], "Breaking Bad");
    assert!(terms
        .numbering
        .contains(&("season".to_string(), "2".to_string())));
    assert!(terms
        .numbering
        .contains(&("ep".to_string(), "5".to_string())));
    assert!(terms
        .ids
        .contains(&("tvdbid".to_string(), "81189".to_string())));
}

#[tokio::test]
async fn tv_match_single_episode_exact() {
    let lib = LibraryId::new();
    let node = episode_ref(lib, 1, 2);
    let candidates = vec![ContentCandidate {
        content_ref: node.clone(),
        title: "The Show".to_string(),
        aliases: vec![],
    }];
    let module = TvModule::new(MockContentLookup { candidates }, MockMetadata::default());

    let mut parsed = ParsedRelease::new("The.Show.S01E02.1080p.WEB-DL");
    parsed.clean_title = Some("The Show".to_string());
    parsed.coordinates = vec![Coordinates::Episode {
        season: 1,
        episode: 2,
        absolute: None,
    }];

    let matches = module.match_release(&parsed).await.expect("match");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].confidence, Confidence::CERTAIN);
    assert_eq!(matches[0].content_ref.id, node.id);
}

#[tokio::test]
async fn tv_match_multi_episode_fans_out_to_each_node_at_full_confidence() {
    // A multi-episode release (S01E01E02) jointly satisfies two DIFFERENT
    // episode nodes; these are not rival interpretations, so neither is demoted.
    let lib = LibraryId::new();
    let e1 = episode_ref(lib, 1, 1);
    let e2 = episode_ref(lib, 1, 2);
    let candidates = vec![
        ContentCandidate {
            content_ref: e1.clone(),
            title: "The Show".to_string(),
            aliases: vec![],
        },
        ContentCandidate {
            content_ref: e2.clone(),
            title: "The Show".to_string(),
            aliases: vec![],
        },
    ];
    let module = TvModule::new(MockContentLookup { candidates }, MockMetadata::default());

    let mut parsed = ParsedRelease::new("The.Show.S01E01E02.1080p");
    parsed.clean_title = Some("The Show".to_string());
    parsed.coordinates = vec![
        Coordinates::Episode {
            season: 1,
            episode: 1,
            absolute: None,
        },
        Coordinates::Episode {
            season: 1,
            episode: 2,
            absolute: None,
        },
    ];

    let matches = module.match_release(&parsed).await.expect("match");
    assert_eq!(matches.len(), 2, "one match per covered episode");
    for m in &matches {
        assert_eq!(
            m.confidence,
            Confidence::CERTAIN,
            "distinct-episode matches keep full confidence"
        );
    }
}

#[tokio::test]
async fn tv_match_wrong_episode_coords_excluded() {
    let lib = LibraryId::new();
    // Library has S01E03; the parse wants S01E02 -> no match.
    let node = episode_ref(lib, 1, 3);
    let candidates = vec![ContentCandidate {
        content_ref: node,
        title: "The Show".to_string(),
        aliases: vec![],
    }];
    let module = TvModule::new(MockContentLookup { candidates }, MockMetadata::default());

    let mut parsed = ParsedRelease::new("The.Show.S01E02");
    parsed.clean_title = Some("The Show".to_string());
    parsed.coordinates = vec![Coordinates::Episode {
        season: 1,
        episode: 2,
        absolute: None,
    }];

    let matches = module.match_release(&parsed).await.expect("match");
    assert!(matches.is_empty(), "episode coords must agree");
}

#[tokio::test]
async fn tv_ambiguous_same_episode_two_series_demoted() {
    // Two different series both titled "The Office" each have an S01E01 node, and
    // the parse title matches both: the SAME coordinate via two nodes = rival
    // interpretations -> demoted to manual.
    let lib = LibraryId::new();
    let us = episode_ref(lib, 1, 1);
    let uk = episode_ref(lib, 1, 1);
    let candidates = vec![
        ContentCandidate {
            content_ref: us,
            title: "The Office".to_string(),
            aliases: vec![],
        },
        ContentCandidate {
            content_ref: uk,
            title: "The Office".to_string(),
            aliases: vec![],
        },
    ];
    let module = TvModule::new(MockContentLookup { candidates }, MockMetadata::default());

    let mut parsed = ParsedRelease::new("The.Office.S01E01");
    parsed.clean_title = Some("The Office".to_string());
    parsed.coordinates = vec![Coordinates::Episode {
        season: 1,
        episode: 1,
        absolute: None,
    }];

    let matches = module.match_release(&parsed).await.expect("match");
    assert_eq!(matches.len(), 2);
    for m in &matches {
        assert!(
            m.confidence.value() <= AMBIGUOUS_CONFIDENCE,
            "rival same-coord matches demoted"
        );
    }
}

#[tokio::test]
async fn tv_match_via_scene_alias() {
    let lib = LibraryId::new();
    let node = episode_ref(lib, 1, 5);
    let candidates = vec![ContentCandidate {
        content_ref: node,
        title: "Attack on Titan".to_string(),
        aliases: vec!["Shingeki no Kyojin".to_string()],
    }];
    let module = TvModule::new(MockContentLookup { candidates }, MockMetadata::default());

    let mut parsed = ParsedRelease::new("Shingeki.no.Kyojin.S01E05");
    parsed.clean_title = Some("Shingeki no Kyojin".to_string());
    parsed.coordinates = vec![Coordinates::Episode {
        season: 1,
        episode: 5,
        absolute: None,
    }];

    let matches = module.match_release(&parsed).await.expect("match");
    assert_eq!(matches.len(), 1);
    assert!(
        matches[0].confidence.value() < 1.0,
        "alias is high not certain"
    );
}

#[tokio::test]
async fn tv_naming_tokens_zero_pad_and_carry_absolute() {
    let lib = LibraryId::new();
    let mut node = episode_ref(lib, 2, 5);
    node.coords = Coordinates::Episode {
        season: 2,
        episode: 5,
        absolute: Some(38),
    };
    let mut meta = MockMetadata::default();
    meta.series
        .insert(node.id, series_meta("Some Anime", &["Sono Anime"]));
    let module = TvModule::new(MockContentLookup { candidates: vec![] }, meta);

    let tokens = module.naming_tokens(&node).await.expect("tokens").tokens;
    let map: HashMap<_, _> = tokens.into_iter().collect();
    assert_eq!(
        map.get("Series Title").map(String::as_str),
        Some("Some Anime")
    );
    assert_eq!(map.get("Season").map(String::as_str), Some("02"));
    assert_eq!(map.get("Episode").map(String::as_str), Some("05"));
    assert_eq!(map.get("Absolute Episode").map(String::as_str), Some("038"));
}

#[tokio::test]
async fn tv_module_reports_its_media_type() {
    let module = TvModule::new(
        MockContentLookup { candidates: vec![] },
        MockMetadata::default(),
    );
    assert_eq!(MediaModule::media_type(&module), MediaType::Tv);
    let _ = ContentId::new(); // keep import used across cfgs
}
