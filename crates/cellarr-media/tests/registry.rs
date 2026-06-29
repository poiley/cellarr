//! Registry wiring and the "new media type" smoke test.
//!
//! Proves the spec's promise: adding a media type is registering one more module
//! that satisfies [`MediaModule`] — nothing else changes, and the pipeline-style
//! call site goes through the registry by [`MediaType`] alone.

mod common;

use std::convert::Infallible;

use async_trait::async_trait;

use cellarr_core::{
    Confidence, ContentId, ContentMatch, ContentRef, Coordinates, LibraryId, MediaModule,
    MediaType, NamingTokens, ParsedRelease, SearchTerms,
};
use cellarr_media::{MediaRegistry, MovieModule, TvModule};

use common::{episode_ref, movie_ref, MockContentLookup, MockMetadata, MovieMeta, SeriesMeta};

#[tokio::test]
async fn registry_dispatches_by_media_type() {
    let lib = LibraryId::new();

    let movie_node = movie_ref(lib);
    let tv_node = episode_ref(lib, 1, 1);

    let mut movie_meta = MockMetadata::default();
    movie_meta.movies.insert(
        movie_node.id,
        MovieMeta {
            title: "A Film".to_string(),
            aliases: vec![],
            year: Some(2020),
            external_ids: vec![],
        },
    );
    let mut tv_meta = MockMetadata::default();
    tv_meta.series.insert(
        tv_node.id,
        SeriesMeta {
            title: "A Series".to_string(),
            aliases: vec![],
            year: Some(2021),
            external_ids: vec![],
        },
    );

    let mut registry = MediaRegistry::new();
    registry.register(MovieModule::new(
        MockContentLookup { candidates: vec![] },
        movie_meta,
    ));
    registry.register(TvModule::new(
        MockContentLookup { candidates: vec![] },
        tv_meta,
    ));

    // The caller has only a ContentRef; it picks the module by media_type.
    let movie_module = registry.get(movie_node.media_type).expect("movie module");
    let terms = movie_module.search_terms(&movie_node).await.expect("terms");
    assert_eq!(terms.queries[0], "A Film 2020");

    let tv_module = registry.get(tv_node.media_type).expect("tv module");
    let terms = tv_module.search_terms(&tv_node).await.expect("terms");
    assert_eq!(terms.queries[0], "A Series");

    // An unregistered type returns None (no panic, no force).
    assert!(registry.get(MediaType::Music).is_none());
}

// --- The "new media type" smoke test ---------------------------------------

/// A minimal stub module for a not-yet-shipped media type (music), implemented
/// only with the trait surface. If this compiles and wires through the registry,
/// the trait is sufficient to add a media type with no other change.
struct StubMusicModule;

#[async_trait]
impl MediaModule for StubMusicModule {
    type Error = Infallible;

    fn media_type(&self) -> MediaType {
        MediaType::Music
    }

    async fn search_terms(&self, _content: &ContentRef) -> Result<SearchTerms, Self::Error> {
        Ok(SearchTerms {
            queries: vec!["An Album".to_string()],
            ids: vec![("mbid".to_string(), "abc-123".to_string())],
            numbering: vec![],
            categories: vec![],
        })
    }

    async fn match_release(
        &self,
        _parsed: &ParsedRelease,
    ) -> Result<Vec<ContentMatch>, Self::Error> {
        Ok(vec![ContentMatch {
            content_ref: ContentRef {
                id: ContentId::new(),
                library_id: LibraryId::new(),
                media_type: MediaType::Music,
                coords: Coordinates::Track { disc: 1, track: 3 },
            },
            confidence: Confidence::CERTAIN,
        }])
    }

    async fn naming_tokens(&self, _content: &ContentRef) -> Result<NamingTokens, Self::Error> {
        Ok(NamingTokens {
            tokens: vec![("Album Title".to_string(), "An Album".to_string())],
        })
    }
}

#[tokio::test]
async fn new_media_type_wires_end_to_end_through_the_trait_only() {
    let mut registry = MediaRegistry::new();
    registry.register(StubMusicModule);

    let module = registry.get(MediaType::Music).expect("music registered");
    assert_eq!(module.media_type(), MediaType::Music);

    let node = ContentRef {
        id: ContentId::new(),
        library_id: LibraryId::new(),
        media_type: MediaType::Music,
        coords: Coordinates::Track { disc: 1, track: 3 },
    };

    let terms = module.search_terms(&node).await.expect("search terms");
    assert_eq!(terms.queries, vec!["An Album".to_string()]);

    let parsed = ParsedRelease::new("An.Album.2020.FLAC");
    let matches = module.match_release(&parsed).await.expect("match");
    assert_eq!(matches.len(), 1);

    let tokens = module.naming_tokens(&node).await.expect("tokens");
    assert!(tokens
        .tokens
        .contains(&("Album Title".to_string(), "An Album".to_string())));

    // Registry now reports two-ish types depending on registration; at minimum
    // Music is present.
    assert!(registry.media_types().any(|t| t == MediaType::Music));
}
