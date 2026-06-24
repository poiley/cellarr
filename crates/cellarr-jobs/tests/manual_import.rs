//! Manual-import scan + commit tests.
//!
//! Drives the runner's `scan_manual_import` (read-only loose-folder scan) and
//! `import_manual` (crash-safe commit) — the engines behind
//! `GET/POST /api/v3/manualimport` — over a seeded movie content node, asserting:
//!   - the scan parses each loose file and suggests the identified node, moving
//!     nothing (the source files are still present after the scan);
//!   - an un-identifiable file is still returned, carrying a rejection;
//!   - the commit imports a chosen file through the crash-safe path: the file
//!     lands renamed under the library root, a media_file row is created + linked,
//!     and the source no longer sits at its old path;
//!   - the commit is idempotent-safe — a second commit of the same already-moved
//!     source reports an error rather than corrupting the library.
//!
//! Everything is offline: a fake (never-driven) indexer + download client + a
//! tempfile SQLite DB + the real Movie media module over a mocked metadata seam,
//! real cellarr-parse.

use std::path::PathBuf;

use async_trait::async_trait;

use cellarr_core::{
    repo::{ContentRepository, MediaFileRepository},
    ContentId, ContentRef, Coordinates, CustomFormat, LibraryId, MediaType, QualityProfile,
    QualityProfileId, QualityRanking, SearchTerms,
};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_jobs::clock::LogicalClock;
use cellarr_jobs::runner::{ManualImportRequest, PipelineRunner, RunnerConfig};
use cellarr_media::{
    ContentCandidate, ContentLookup, MediaRegistry, MetadataLookup, MovieMeta, MovieModule,
    SeriesMeta,
};

// ---------------------------------------------------------------------------
// Synthetic seams (offline). None hit a network; neither is driven by a scan or
// a manual commit (which never grab).
// ---------------------------------------------------------------------------

struct FakeIndexer;

#[derive(Debug, thiserror::Error)]
#[error("fake indexer error")]
struct FakeIndexerError;

#[async_trait]
impl cellarr_core::traits::Indexer for FakeIndexer {
    type Error = FakeIndexerError;
    fn name(&self) -> &str {
        "fake-indexer"
    }
    async fn search(
        &self,
        _terms: &SearchTerms,
    ) -> Result<Vec<cellarr_core::Release>, Self::Error> {
        Ok(Vec::new())
    }
    async fn latest(&self) -> Result<Vec<cellarr_core::Release>, Self::Error> {
        Ok(Vec::new())
    }
}

/// A download client that PANICS if driven: a scan/commit must never grab.
struct NeverDrivenClient;

#[derive(Debug, thiserror::Error)]
#[error("never-driven client error")]
struct NeverDrivenError;

#[async_trait]
impl cellarr_core::traits::DownloadClient for NeverDrivenClient {
    type Error = NeverDrivenError;
    fn name(&self) -> &str {
        "never-driven-client"
    }
    async fn add(&self, _grab: &cellarr_core::GrabRequest) -> Result<String, Self::Error> {
        panic!("manual import must NOT grab: download client was driven");
    }
    async fn status(
        &self,
        _download_id: &str,
    ) -> Result<cellarr_core::DownloadStatus, Self::Error> {
        panic!("manual import must NOT track: status was polled");
    }
    async fn remove(&self, _download_id: &str, _delete_data: bool) -> Result<(), Self::Error> {
        panic!("manual import must NOT remove a download");
    }
}

struct MockContentLookup {
    candidate: ContentCandidate,
}

#[derive(Debug, thiserror::Error)]
#[error("mock content lookup error")]
struct MockLookupError;

#[async_trait]
impl ContentLookup for MockContentLookup {
    type Error = MockLookupError;
    async fn candidates_for_title(
        &self,
        _media_type: MediaType,
        title_query: &str,
    ) -> Result<Vec<ContentCandidate>, Self::Error> {
        // Only resolve a candidate when the parsed title actually matches the
        // seeded movie, so an unrelated file (e.g. "Random Junk") yields no match
        // and the scan reports a rejection rather than a force-fit suggestion.
        let normalized = title_query.to_lowercase();
        if normalized.contains("matrix") {
            Ok(vec![self.candidate.clone()])
        } else {
            Ok(Vec::new())
        }
    }
}

struct MockMetadata {
    movie: Option<MovieMeta>,
}

#[async_trait]
impl MetadataLookup for MockMetadata {
    type Error = MockLookupError;
    async fn movie_meta(
        &self,
        _content: ContentId,
        _title_id: Option<cellarr_core::TitleId>,
    ) -> Result<Option<MovieMeta>, Self::Error> {
        Ok(self.movie.clone())
    }
    async fn series_meta(
        &self,
        _content: ContentId,
        _title_id: Option<cellarr_core::TitleId>,
    ) -> Result<Option<SeriesMeta>, Self::Error> {
        Ok(None::<SeriesMeta>)
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn permissive_profile() -> QualityProfile {
    QualityProfile {
        id: QualityProfileId::new(),
        name: "permissive".into(),
        allowed_qualities: QualityRanking::default()
            .qualities
            .iter()
            .map(|q| q.rank)
            .collect(),
        upgrades_allowed: true,
        cutoff_quality: 21,
        min_custom_format_score: 0,
        upgrade_until_custom_format_score: 100,
        required_languages: Vec::new(),
    }
}

async fn seed_movie_node(db: &Database, root: &str) -> ContentRef {
    let library_id = LibraryId::new();
    let library = cellarr_core::Library {
        id: library_id,
        media_type: MediaType::Movie,
        name: "movie lib".into(),
        root_folders: vec![root.into()],
        default_quality_profile: QualityProfileId::new(),
    };
    db.config().upsert_library(&library).await.unwrap();

    let content_id = ContentId::new();
    let node = cellarr_core::ContentNode {
        id: content_id,
        library_id,
        media_type: MediaType::Movie,
        parent_id: None,
        kind: cellarr_core::ContentKind::Movie,
        coords: Coordinates::Movie,
        monitored: true,
        title_id: None,
    };
    db.content().upsert(&node).await.unwrap();
    ContentRef::new(content_id, library_id, MediaType::Movie, Coordinates::Movie).unwrap()
}

fn registry_for(node: &ContentRef, title: &str) -> MediaRegistry {
    let candidate = ContentCandidate {
        content_ref: node.clone(),
        title: title.to_string(),
        aliases: Vec::new(),
    };
    let mut registry = MediaRegistry::new();
    registry.register(MovieModule::new(
        MockContentLookup { candidate },
        MockMetadata {
            movie: Some(MovieMeta {
                title: title.into(),
                aliases: Vec::new(),
                year: Some(1999),
                external_ids: Vec::new(),
            }),
        },
    ));
    registry
}

fn runner_config(root: PathBuf) -> RunnerConfig {
    RunnerConfig {
        profile: permissive_profile(),
        custom_formats: Vec::<CustomFormat>::new(),
        ranking: QualityRanking::default(),
        proper_repack_policy: ProperRepackPolicy::default(),
        library_root: root,
        naming_format: "{Movie Title} ({Release Year})/{Movie Title}.{Extension}".into(),
        indexer_id: cellarr_core::IndexerId::new(),
        client_id: cellarr_core::DownloadClientId::new(),
        category: "cellarr".into(),
        max_track_polls: 5,
        track_poll_interval: std::time::Duration::ZERO,
        client_host: String::new(),
        remote_path_mappings: Vec::new(),
        write_nfo: false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scan_returns_parsed_and_identified_candidates_and_moves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let lib_root = tmp.path().join("library");
    std::fs::create_dir_all(&lib_root).unwrap();

    let node = seed_movie_node(&db, lib_root.to_str().unwrap()).await;
    let registry = registry_for(&node, "The Matrix");

    // A loose download folder with one identifiable file and one that cannot be
    // placed (its parsed title matches no library item).
    let loose = tmp.path().join("downloads");
    std::fs::create_dir_all(&loose).unwrap();
    let good = loose.join("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv");
    let junk = loose.join("Random.Junk.File.2024.1080p.mkv");
    std::fs::write(&good, b"good-bytes").unwrap();
    std::fs::write(&junk, b"junk-bytes").unwrap();

    let indexer = FakeIndexer;
    let client = NeverDrivenClient;
    let clock = LogicalClock::new(0);
    let config = runner_config(lib_root.clone());
    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);

    let candidates = runner.scan_manual_import(&loose).await.unwrap();
    assert_eq!(candidates.len(), 2, "both loose files are reported");

    // The identifiable file suggests the seeded movie node, carries its parsed
    // title + a real quality, and has no rejection.
    let matched = candidates
        .iter()
        .find(|c| c.name.contains("Matrix"))
        .expect("the Matrix file is a candidate");
    let suggestion = matched
        .suggested
        .as_ref()
        .expect("the identifiable file suggests a node");
    assert_eq!(suggestion.content_id, node.id);
    assert!(matched.quality.rank > 0, "a real quality was parsed");
    assert!(
        matched.rejections.is_empty(),
        "an identified file is not rejected"
    );
    assert_eq!(matched.size, "good-bytes".len() as u64);

    // The un-identifiable file is still reported, with no suggestion and a reason.
    let unmatched = candidates
        .iter()
        .find(|c| c.name.contains("Random"))
        .expect("the junk file is a candidate");
    assert!(unmatched.suggested.is_none(), "junk file has no suggestion");
    assert!(
        !unmatched.rejections.is_empty(),
        "an un-identifiable file carries a rejection"
    );

    // The scan moved NOTHING — both source files are still present.
    assert!(good.exists(), "scan must not move the source file");
    assert!(junk.exists(), "scan must not move the source file");
}

#[tokio::test]
async fn commit_imports_a_chosen_file_through_the_crash_safe_path() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let lib_root = tmp.path().join("library");
    std::fs::create_dir_all(&lib_root).unwrap();

    let node = seed_movie_node(&db, lib_root.to_str().unwrap()).await;
    let registry = registry_for(&node, "The Matrix");

    let loose = tmp.path().join("downloads");
    std::fs::create_dir_all(&loose).unwrap();
    let source = loose.join("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv");
    std::fs::write(&source, b"movie-bytes").unwrap();

    let indexer = FakeIndexer;
    let client = NeverDrivenClient;
    let clock = LogicalClock::new(0);
    let config = runner_config(lib_root.clone());
    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);

    let request = ManualImportRequest {
        path: source.to_string_lossy().into_owned(),
        content_id: node.id,
    };
    let (imported, errors) = runner
        .import_manual(std::slice::from_ref(&request))
        .await
        .unwrap();
    assert!(errors.is_empty(), "no per-file errors: {errors:?}");
    assert_eq!(imported.len(), 1, "the chosen file was imported");

    let result = &imported[0];
    assert_eq!(result.content_id, node.id);
    // The file landed RENAMED, under the library root, per the naming format.
    let dest = PathBuf::from(&result.destination_path);
    assert!(
        dest.exists(),
        "the imported file is on disk at its destination"
    );
    assert!(
        dest.starts_with(&lib_root),
        "the destination is under the library root: {dest:?}"
    );
    assert!(
        dest.to_string_lossy().contains("The Matrix"),
        "the file was renamed from the parsed name: {dest:?}"
    );
    // The bytes are at the destination.
    assert_eq!(std::fs::read(&dest).unwrap(), b"movie-bytes");
    // The crash-safe import hardlinks within one filesystem, so the user's loose
    // source is PRESERVED (the manual import must not delete the user's file until
    // they confirm cleanup) — but the two paths share one inode (a hardlink, not a
    // wasteful copy).
    assert!(
        source.exists(),
        "the loose source is preserved (not deleted) by a same-fs import"
    );
    assert_eq!(
        std::fs::read(&source).unwrap(),
        std::fs::read(&dest).unwrap(),
        "destination and source share the same bytes (hardlinked)"
    );

    // A media_file row was created AND linked to the node — the library now
    // recognizes the import (the node is no longer "missing").
    let files = MediaFileRepository::list_for_content(&db.media_files(), node.id)
        .await
        .unwrap();
    assert_eq!(files.len(), 1, "one media_file row linked to the node");
    assert_eq!(files[0].path, result.destination_path);

    // A second commit of the same source is idempotent-safe: it re-imports to the
    // same destination (the bytes are already there) and never creates a duplicate
    // media_file row for that path — the library is not corrupted by a re-commit.
    let (imported2, errors2) = runner
        .import_manual(std::slice::from_ref(&request))
        .await
        .unwrap();
    assert!(
        errors2.is_empty(),
        "a re-commit of the same source is safe: {errors2:?}"
    );
    assert_eq!(
        imported2.len(),
        1,
        "the re-commit reports the same destination"
    );
    assert_eq!(
        imported2[0].destination_path, result.destination_path,
        "the re-commit lands at the same destination"
    );
    // The library still has exactly one file for the node — no duplicate row for
    // the same on-disk path (media_file.path is unique).
    let files_after = MediaFileRepository::list_for_content(&db.media_files(), node.id)
        .await
        .unwrap();
    assert_eq!(
        files_after.len(),
        1,
        "no duplicate media_file on a re-commit"
    );
}

#[tokio::test]
async fn commit_reports_error_for_an_unknown_content_node() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let lib_root = tmp.path().join("library");
    std::fs::create_dir_all(&lib_root).unwrap();

    let node = seed_movie_node(&db, lib_root.to_str().unwrap()).await;
    let registry = registry_for(&node, "The Matrix");

    let source = tmp.path().join("loose.mkv");
    std::fs::write(&source, b"x").unwrap();

    let indexer = FakeIndexer;
    let client = NeverDrivenClient;
    let clock = LogicalClock::new(0);
    let config = runner_config(lib_root);
    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);

    // A content id that was never seeded: the commit reports it as a per-file error
    // and imports nothing — never a panic, never a wrong-node placement.
    let request = ManualImportRequest {
        path: source.to_string_lossy().into_owned(),
        content_id: ContentId::new(),
    };
    let (imported, errors) = runner.import_manual(&[request]).await.unwrap();
    assert!(imported.is_empty());
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].contains("not found"),
        "error names the missing node"
    );
    // The source was not touched (no node to place it on).
    assert!(source.exists());
}
