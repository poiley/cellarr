//! The interactive (manual) grab test.
//!
//! Drives the runner's `grab_release` path — the engine behind
//! `POST /api/v3/release` — for a seeded movie content node through a FAKE indexer
//! offering several releases, and asserts:
//!   - grabbing a release by its `guid` drives the FULL Grab→Track→Import path
//!     (unlike the read-only preview, which never grabs), landing the file on disk
//!     and recording the grab + history,
//!   - the **user's pick** is honored: grabbing a lower-quality release by guid
//!     grabs THAT release even though a better one was offered (the override the
//!     interactive screen relies on),
//!   - grabbing a guid the indexers no longer offer is a benign `NothingFound`,
//!     not an error, and grabs nothing.
//!
//! Offline: a fake indexer + a fake completing download client + a tempfile SQLite
//! DB + the real Movie media module over mocked metadata, real cellarr-parse +
//! cellarr-decide.

use std::path::PathBuf;

use async_trait::async_trait;

use cellarr_core::{
    repo::{ContentRepository, GrabRepository, HistoryRepository},
    ContentId, ContentRef, Coordinates, CustomFormat, GrabStatus, LibraryId, MediaType, Protocol,
    QualityProfile, QualityProfileId, QualityRanking, Release, SearchTerms,
};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_jobs::clock::LogicalClock;
use cellarr_jobs::runner::{PipelineRunner, RunOutcome, RunnerConfig};
use cellarr_media::{
    ContentCandidate, ContentLookup, MediaRegistry, MetadataLookup, MovieMeta, MovieModule,
    SeriesMeta,
};

// ---------------------------------------------------------------------------
// Synthetic seams (offline). None hit a network.
// ---------------------------------------------------------------------------

struct FakeIndexer {
    releases: Vec<Release>,
}

#[derive(Debug, thiserror::Error)]
#[error("fake indexer error")]
struct FakeIndexerError;

#[async_trait]
impl cellarr_core::traits::Indexer for FakeIndexer {
    type Error = FakeIndexerError;
    fn name(&self) -> &str {
        "fake-indexer"
    }
    async fn search(&self, _terms: &SearchTerms) -> Result<Vec<Release>, Self::Error> {
        Ok(self.releases.clone())
    }
    async fn latest(&self) -> Result<Vec<Release>, Self::Error> {
        Ok(self.releases.clone())
    }
}

/// A fake download client that immediately "completes" every download at the
/// pre-populated `content_path`, recording the title it was handed so the test can
/// assert WHICH release was grabbed.
struct RecordingClient {
    content_path: String,
    grabbed_title: std::sync::Mutex<Option<String>>,
}

#[derive(Debug, thiserror::Error)]
#[error("recording client error")]
struct RecordingClientError;

#[async_trait]
impl cellarr_core::traits::DownloadClient for RecordingClient {
    type Error = RecordingClientError;
    fn name(&self) -> &str {
        "recording-client"
    }
    async fn add(&self, grab: &cellarr_core::GrabRequest) -> Result<String, Self::Error> {
        *self.grabbed_title.lock().unwrap() = Some(grab.release.title.clone());
        Ok("dl-1".to_string())
    }
    async fn status(
        &self,
        _download_id: &str,
    ) -> Result<cellarr_core::DownloadStatus, Self::Error> {
        Ok(cellarr_core::DownloadStatus {
            state: cellarr_core::DownloadState::Completed,
            progress: 1.0,
            content_path: Some(self.content_path.clone()),
            ratio: Some(1.0),
            seeding_time_secs: Some(0),
            peers: Some(10),
            error_string: None,
        })
    }
    async fn remove(&self, _download_id: &str, _delete_data: bool) -> Result<(), Self::Error> {
        Ok(())
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
        _title_query: &str,
    ) -> Result<Vec<ContentCandidate>, Self::Error> {
        Ok(vec![self.candidate.clone()])
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

/// A permissive profile allowing every real quality (rank > 0).
fn permissive_profile() -> QualityProfile {
    let ranking = QualityRanking::default();
    let allowed: Vec<u32> = ranking
        .qualities
        .iter()
        .map(|q| q.rank)
        .filter(|r| *r != 0)
        .collect();
    QualityProfile {
        id: QualityProfileId::new(),
        name: "permissive".into(),
        allowed_qualities: allowed,
        upgrades_allowed: true,
        cutoff_quality: 21,
        min_custom_format_score: 0,
        upgrade_until_custom_format_score: 100,
        required_languages: Vec::new(),
    }
}

fn movie_release(title: &str, guid: &str) -> Release {
    Release {
        indexer_id: cellarr_core::IndexerId::new(),
        title: title.to_string(),
        download_url: format!("magnet:?xt={guid}"),
        guid: Some(guid.to_string()),
        protocol: Protocol::Torrent,
        size: Some(8_000_000_000),
        seeders: Some(50),
        indexer_flags: Vec::new(),
    }
}

async fn seed_movie_node(db: &Database) -> ContentRef {
    let library_id = LibraryId::new();
    let library = cellarr_core::Library {
        id: library_id,
        media_type: MediaType::Movie,
        name: "movie lib".into(),
        root_folders: vec!["/tmp/synthetic".into()],
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

fn runner_config(library_root: PathBuf) -> RunnerConfig {
    RunnerConfig {
        profile: permissive_profile(),
        custom_formats: Vec::<CustomFormat>::new(),
        ranking: QualityRanking::default(),
        proper_repack_policy: ProperRepackPolicy::default(),
        library_root,
        naming_format: "{Movie Title} ({Release Year})/{Movie Title}.{Extension}".into(),
        indexer_id: cellarr_core::IndexerId::new(),
        client_id: cellarr_core::DownloadClientId::new(),
        category: "cellarr".into(),
        max_track_polls: 5,
        track_poll_interval: std::time::Duration::ZERO,
        client_host: String::new(),
        remote_path_mappings: Vec::new(),
        write_nfo: false,
        delay_profiles: Vec::new(),
        content_tags: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grab_release_grabs_the_users_pick_and_imports_it() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("cellarr.sqlite");
    let db = Database::open(db_path.to_str().unwrap()).await.unwrap();

    // A real downloaded file the fake client points at.
    let download_dir = tmp.path().join("downloads/movie");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("The.Matrix.1999.720p.WEB-DL.x264-GROUP.mkv");
    std::fs::write(&downloaded, b"synthetic movie bytes").unwrap();
    let library_root = tmp.path().join("library/movies");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_movie_node(&db).await;
    let registry = registry_for(&node, "The Matrix");

    // The indexer offers a BETTER 1080p release and a lower 720p release. The user
    // picks the 720p by guid — the grab must honor that pick, not the best one.
    let indexer = FakeIndexer {
        releases: vec![
            movie_release("The.Matrix.1999.1080p.BluRay.x264-GROUP", "guid-1080p"),
            movie_release("The.Matrix.1999.720p.WEB-DL.x264-GROUP", "guid-720p"),
        ],
    };
    let client = RecordingClient {
        content_path: downloaded.to_string_lossy().into_owned(),
        grabbed_title: std::sync::Mutex::new(None),
    };
    let clock = LogicalClock::new(0);
    let config = runner_config(library_root.clone());

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
    let outcome = runner.grab_release(&node, "guid-720p").await.unwrap();

    let (grab_id, destinations) = match outcome {
        RunOutcome::Imported {
            grab_id,
            destinations,
        } => (grab_id, destinations),
        other => panic!("expected Imported, got {other:?}"),
    };

    // The user's exact pick (the 720p release) was handed to the client, NOT the
    // better 1080p one the engine would otherwise prefer.
    let grabbed = client.grabbed_title.lock().unwrap().clone().unwrap();
    assert!(
        grabbed.contains("720p"),
        "the user's pick (720p) must be grabbed, got {grabbed:?}"
    );

    // The file actually landed in the library and the grab reached Imported.
    assert_eq!(destinations.len(), 1);
    let dest = PathBuf::from(&destinations[0]);
    assert!(dest.exists(), "imported file must exist at {dest:?}");
    assert!(dest.starts_with(&library_root));
    let grab = GrabRepository::get(&db.grabs(), grab_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grab.status, GrabStatus::Imported);

    // History records the grab + import (unlike the read-only preview).
    let history = HistoryRepository::for_content(&db.history(), node.id)
        .await
        .unwrap();
    assert!(
        !history.is_empty(),
        "an interactive grab must record history"
    );
}

#[tokio::test]
async fn grab_release_for_unknown_guid_is_nothing_found_and_grabs_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("cellarr.sqlite");
    let db = Database::open(db_path.to_str().unwrap()).await.unwrap();

    let download_dir = tmp.path().join("downloads/movie");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("anything.mkv");
    std::fs::write(&downloaded, b"bytes").unwrap();
    let library_root = tmp.path().join("library/movies");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_movie_node(&db).await;
    let registry = registry_for(&node, "The Matrix");

    let indexer = FakeIndexer {
        releases: vec![movie_release(
            "The.Matrix.1999.1080p.BluRay.x264-GROUP",
            "guid-1080p",
        )],
    };
    let client = RecordingClient {
        content_path: downloaded.to_string_lossy().into_owned(),
        grabbed_title: std::sync::Mutex::new(None),
    };
    let clock = LogicalClock::new(0);
    let config = runner_config(library_root);

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
    let outcome = runner
        .grab_release(&node, "guid-that-does-not-exist")
        .await
        .unwrap();

    assert!(
        matches!(outcome, RunOutcome::NothingFound),
        "an unknown guid is NothingFound, got {outcome:?}"
    );
    // Nothing was grabbed.
    assert!(
        client.grabbed_title.lock().unwrap().is_none(),
        "no release should be grabbed for an unknown guid"
    );
    let history = HistoryRepository::for_content(&db.history(), node.id)
        .await
        .unwrap();
    assert!(history.is_empty(), "no grab => no history");
}
