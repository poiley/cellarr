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

// --- season units ----------------------------------------------------------

/// A season pack addresses the season unit it fills. Before seasons carried a
/// season coordinate this could not match anything, so a pack found by any search
/// was silently discarded.
#[tokio::test]
async fn tv_match_season_pack_lands_on_the_season_unit() {
    let lib = LibraryId::new();
    let season6 = common::season_ref(lib, 6);
    let candidates = vec![ContentCandidate {
        content_ref: season6.clone(),
        title: "Love Island".to_string(),
        aliases: vec![],
    }];
    let module = TvModule::new(MockContentLookup { candidates }, MockMetadata::default());

    let mut parsed = ParsedRelease::new("Love.Island.S06.1080p.WEB-DL.x264-GROUP");
    parsed.clean_title = Some("Love Island".to_string());
    parsed.coordinates = vec![Coordinates::SeasonPack { season: 6 }];

    let matches = module.match_release(&parsed).await.expect("match");
    assert_eq!(matches.len(), 1, "{matches:?}");
    assert_eq!(matches[0].content_ref.id, season6.id);
}

/// A pack for a different season must not land on this one.
#[tokio::test]
async fn tv_match_season_pack_ignores_a_different_season() {
    let lib = LibraryId::new();
    let candidates = vec![ContentCandidate {
        content_ref: common::season_ref(lib, 6),
        title: "Love Island".to_string(),
        aliases: vec![],
    }];
    let module = TvModule::new(MockContentLookup { candidates }, MockMetadata::default());

    let mut parsed = ParsedRelease::new("Love.Island.S02.1080p.WEB-DL.x264-GROUP");
    parsed.clean_title = Some("Love Island".to_string());
    parsed.coordinates = vec![Coordinates::SeasonPack { season: 2 }];

    assert!(module
        .match_release(&parsed)
        .await
        .expect("match")
        .is_empty());
}

/// A multi-season pack carries one coordinate per covered season, so it satisfies
/// every season unit it spans — and none it doesn't.
#[tokio::test]
async fn tv_match_multi_season_pack_lands_on_every_season_it_covers() {
    let lib = LibraryId::new();
    let s1 = common::season_ref(lib, 1);
    let s2 = common::season_ref(lib, 2);
    let s3 = common::season_ref(lib, 3);
    let s9 = common::season_ref(lib, 9);
    let candidates = [&s1, &s2, &s3, &s9]
        .into_iter()
        .map(|r| ContentCandidate {
            content_ref: r.clone(),
            title: "Love Island".to_string(),
            aliases: vec![],
        })
        .collect();
    let module = TvModule::new(MockContentLookup { candidates }, MockMetadata::default());

    let mut parsed = ParsedRelease::new("Love.Island.S01-S03.COMPLETE.1080p.WEB-DL.x264-GROUP");
    parsed.clean_title = Some("Love Island".to_string());
    parsed.coordinates = vec![
        Coordinates::SeasonPack { season: 1 },
        Coordinates::SeasonPack { season: 2 },
        Coordinates::SeasonPack { season: 3 },
    ];

    let matches = module.match_release(&parsed).await.expect("match");
    let ids: Vec<_> = matches.iter().map(|m| m.content_ref.id).collect();
    assert_eq!(ids.len(), 3, "{matches:?}");
    for wanted in [s1.id, s2.id, s3.id] {
        assert!(ids.contains(&wanted), "missing a covered season: {ids:?}");
    }
    assert!(!ids.contains(&s9.id), "season 9 is not covered: {ids:?}");
}

/// A season unit searches by season alone — never with an episode number, which
/// is what the old episode-zero sentinel produced.
#[tokio::test]
async fn tv_season_unit_search_terms_carry_season_without_an_episode() {
    let lib = LibraryId::new();
    let node = common::season_ref(lib, 6);
    let mut meta = MockMetadata::default();
    meta.series.insert(
        node.id,
        SeriesMeta {
            title: "Love Island".to_string(),
            aliases: vec![],
            year: None,
            external_ids: vec![],
        },
    );
    let module = TvModule::new(MockContentLookup { candidates: vec![] }, meta);

    let terms = module.search_terms(&node).await.expect("search terms");
    assert!(terms
        .numbering
        .contains(&("season".to_string(), "6".to_string())));
    assert!(
        !terms.numbering.iter().any(|(k, _)| k == "ep"),
        "a season unit must not ask for an episode: {:?}",
        terms.numbering
    );
}

/// A pack fills the season unit *and* covers that season's episodes — the grab
/// path fans it out to them, the adopt path sees its files land on them — while
/// staying off episodes of seasons it does not cover.
#[tokio::test]
async fn tv_match_season_pack_covers_its_episodes_but_not_another_seasons() {
    let lib = LibraryId::new();
    let unit = common::season_ref(lib, 6);
    let s6e1 = common::episode_ref(lib, 6, 1);
    let s6e2 = common::episode_ref(lib, 6, 2);
    let s2e1 = common::episode_ref(lib, 2, 1);
    let candidates = [&unit, &s6e1, &s6e2, &s2e1]
        .into_iter()
        .map(|r| ContentCandidate {
            content_ref: r.clone(),
            title: "Love Island".to_string(),
            aliases: vec![],
        })
        .collect();
    let module = TvModule::new(MockContentLookup { candidates }, MockMetadata::default());

    let mut parsed = ParsedRelease::new("Love.Island.S06.1080p.WEB-DL.x264-GROUP");
    parsed.clean_title = Some("Love Island".to_string());
    parsed.coordinates = vec![Coordinates::SeasonPack { season: 6 }];

    let ids: Vec<_> = module
        .match_release(&parsed)
        .await
        .expect("match")
        .into_iter()
        .map(|m| m.content_ref.id)
        .collect();
    assert!(ids.contains(&unit.id), "the season unit: {ids:?}");
    assert!(ids.contains(&s6e1.id), "S06E01 is covered: {ids:?}");
    assert!(ids.contains(&s6e2.id), "S06E02 is covered: {ids:?}");
    assert!(!ids.contains(&s2e1.id), "S02E01 is not covered: {ids:?}");
}
