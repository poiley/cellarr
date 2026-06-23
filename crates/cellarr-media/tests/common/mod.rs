//! Shared test mocks and corpus loaders.
//!
//! Everything here is test-only: in-memory implementations of the seam traits
//! (`ContentLookup`, `MetadataLookup`, `SceneMappingProvider`) so the modules
//! and Identify can be exercised without `cellarr-meta`, `cellarr-db`, or any
//! live service — exactly the "mock the MetadataSource/scene-mapping" the spec
//! calls for.
//!
//! Each integration test file compiles this module independently and uses only
//! the subset it needs, so unused items are expected here.
#![allow(dead_code)]

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use cellarr_core::{ContentId, ContentRef, Coordinates, LibraryId, MediaType, TitleId};
use cellarr_media::content::{ContentCandidate, ContentLookup};
use cellarr_media::identify::{SceneMapping, SceneMappingProvider, SceneRange};
use cellarr_media::meta::MetadataLookup;
pub use cellarr_media::meta::{MovieMeta, SeriesMeta};

/// A content lookup backed by a fixed candidate list, filtered by media type.
/// `candidates_for_title` returns every candidate of the type (the module does
/// the title/coordinate refinement), which is the loosest a real index could be.
pub struct MockContentLookup {
    pub candidates: Vec<ContentCandidate>,
}

#[async_trait]
impl ContentLookup for MockContentLookup {
    type Error = Infallible;

    async fn candidates_for_title(
        &self,
        media_type: MediaType,
        _title_query: &str,
    ) -> Result<Vec<ContentCandidate>, Self::Error> {
        Ok(self
            .candidates
            .iter()
            .filter(|c| c.content_ref.media_type == media_type)
            .cloned()
            .collect())
    }
}

/// A metadata lookup keyed by content id.
#[derive(Default)]
pub struct MockMetadata {
    pub movies: HashMap<ContentId, MovieMeta>,
    pub series: HashMap<ContentId, SeriesMeta>,
}

#[async_trait]
impl MetadataLookup for MockMetadata {
    type Error = Infallible;

    async fn movie_meta(
        &self,
        content: ContentId,
        _title_id: Option<TitleId>,
    ) -> Result<Option<MovieMeta>, Self::Error> {
        Ok(self.movies.get(&content).cloned())
    }

    async fn series_meta(
        &self,
        content: ContentId,
        _title_id: Option<TitleId>,
    ) -> Result<Option<SeriesMeta>, Self::Error> {
        Ok(self.series.get(&content).cloned())
    }
}

/// A scene-mapping provider backed by an external-id → mapping table.
#[derive(Default)]
pub struct MockSceneProvider {
    pub mappings: HashMap<String, SceneMapping>,
}

#[async_trait]
impl SceneMappingProvider for MockSceneProvider {
    type Error = Infallible;

    async fn scene_mapping(
        &self,
        series_external_id: &str,
    ) -> Result<Option<SceneMapping>, Self::Error> {
        Ok(self.mappings.get(series_external_id).cloned())
    }
}

/// Build a TV episode `ContentRef` for a fresh node.
pub fn episode_ref(library: LibraryId, season: u32, episode: u32) -> ContentRef {
    ContentRef {
        id: ContentId::new(),
        library_id: library,
        media_type: MediaType::Tv,
        coords: Coordinates::Episode {
            season,
            episode,
            absolute: None,
        },
    }
}

/// Build a movie `ContentRef` for a fresh node.
pub fn movie_ref(library: LibraryId) -> ContentRef {
    ContentRef {
        id: ContentId::new(),
        library_id: library,
        media_type: MediaType::Movie,
        coords: Coordinates::Movie,
    }
}

// ---------------------------------------------------------------------------
// Corpus loading: corpus/anime/*.toml
// ---------------------------------------------------------------------------

/// The repo's `corpus/anime` directory, resolved relative to this crate.
pub fn anime_corpus_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/cellarr-media; corpus is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("anime")
}

/// One mapping range as it appears in a corpus vector.
#[derive(Debug, Deserialize, Clone)]
pub struct CorpusRange {
    pub season: u32,
    pub start_absolute: u32,
    pub length: u32,
}

impl CorpusRange {
    pub fn into_scene_range(self) -> SceneRange {
        SceneRange {
            season: self.season,
            start_absolute: self.start_absolute,
            length: self.length,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct CorpusMapping {
    pub ranges: Vec<CorpusRange>,
}

/// A successful absolute→season/episode mapping vector.
#[derive(Debug, Deserialize)]
pub struct MappingCase {
    pub series: String,
    pub external_id: String,
    pub absolute: u32,
    #[allow(dead_code)]
    pub source: String,
    pub mapping: CorpusMapping,
    pub expected: MappingExpected,
}

#[derive(Debug, Deserialize)]
pub struct MappingExpected {
    pub season: u32,
    pub episode: u32,
}

#[derive(Debug, Deserialize)]
struct MappingFile {
    case: Vec<MappingCase>,
}

/// An unmapped/malformed vector.
#[derive(Debug, Deserialize)]
pub struct UnmappedCase {
    pub series: String,
    pub external_id: String,
    pub absolute: u32,
    #[allow(dead_code)]
    pub source: String,
    pub mapping: CorpusMapping,
    pub outcome: Outcome,
}

#[derive(Debug, Deserialize)]
pub struct Outcome {
    /// "unmapped" or "malformed".
    pub kind: String,
}

#[derive(Debug, Deserialize)]
struct UnmappedFile {
    case: Vec<UnmappedCase>,
}

/// Load the positive absolute→season/episode mapping vectors.
pub fn load_mapping_cases() -> Vec<MappingCase> {
    let path = anime_corpus_dir().join("absolute_to_season_episode.toml");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let file: MappingFile =
        toml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    file.case
}

/// Load the unmapped/malformed vectors.
pub fn load_unmapped_cases() -> Vec<UnmappedCase> {
    let path = anime_corpus_dir().join("unmapped_absolute.toml");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let file: UnmappedFile =
        toml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    file.case
}

/// Build a one-series [`MockSceneProvider`] from a corpus mapping.
pub fn provider_for(external_id: &str, series: &str, mapping: &CorpusMapping) -> MockSceneProvider {
    let scene = SceneMapping {
        series: series.to_string(),
        ranges: mapping
            .ranges
            .iter()
            .cloned()
            .map(CorpusRange::into_scene_range)
            .collect(),
    };
    let mut mappings = HashMap::new();
    mappings.insert(external_id.to_string(), scene);
    MockSceneProvider { mappings }
}
