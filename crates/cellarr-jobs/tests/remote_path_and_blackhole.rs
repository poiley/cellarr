//! Two generic-download wins, exercised through the *real* pipeline runner:
//!
//! 1. **Blackhole / watch-folder handoff.** The real [`BlackholeClient`] is wired
//!    in as the runner's download client. We drop a job into the watch dir via
//!    `add`, *simulate the external client* by placing finished files in the
//!    completed dir, and assert Track → Import drives the run to `Imported` with
//!    the files landing on disk — proving the universal adapter hands off to
//!    Import exactly like an API client.
//!
//! 2. **Remote-path mapping (shared layer).** A client that reports a
//!    `content_path` under a *remote* prefix (as it would when running on another
//!    host) is rewritten to the *local* prefix before Import. With a matching
//!    mapping the import succeeds; an unmapped path passes through unchanged (and,
//!    being non-existent locally, holds for review).

use std::path::PathBuf;

use async_trait::async_trait;

use cellarr_core::{
    ContentId, ContentRef, Coordinates, CustomFormat, LibraryId, MediaType, Protocol,
    QualityProfile, QualityProfileId, QualityRanking, Release, RemotePathMapping, SearchTerms,
};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_download::{BlackholeClient, BlackholeSettings};
use cellarr_jobs::clock::LogicalClock;
use cellarr_jobs::runner::{PipelineRunner, RunOutcome, RunnerConfig};
use cellarr_media::{
    ContentCandidate, ContentLookup, MediaRegistry, MetadataLookup, MovieMeta, MovieModule,
    SeriesMeta,
};

// ---------------------------------------------------------------------------
// Synthetic seams (offline; no network).
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

/// A download client that completes immediately at a fixed `content_path` — used
/// to exercise the remote-path rewrite in isolation (the path it reports is the
/// *client's* view, which the mapping must translate).
struct ReportingClient {
    reported_path: String,
}

#[derive(Debug, thiserror::Error)]
#[error("fake client error")]
struct FakeClientError;

#[async_trait]
impl cellarr_core::traits::DownloadClient for ReportingClient {
    type Error = FakeClientError;
    fn name(&self) -> &str {
        "reporting-client"
    }
    async fn add(&self, _grab: &cellarr_core::GrabRequest) -> Result<String, Self::Error> {
        Ok("dl-1".to_string())
    }
    async fn status(
        &self,
        _download_id: &str,
    ) -> Result<cellarr_core::DownloadStatus, Self::Error> {
        Ok(cellarr_core::DownloadStatus {
            state: cellarr_core::DownloadState::Completed,
            progress: 1.0,
            content_path: Some(self.reported_path.clone()),
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
#[error("mock lookup error")]
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
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

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
        cutoff_quality: 14,
        min_custom_format_score: 0,
        upgrade_until_custom_format_score: 100,
        required_languages: Vec::new(),
    }
}

fn movie_release(title: &str, download_url: &str) -> Release {
    Release {
        indexer_id: cellarr_core::IndexerId::new(),
        title: title.to_string(),
        download_url: download_url.to_string(),
        guid: Some("guid-movie".into()),
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
        name: "Movie lib".into(),
        root_folders: vec!["/tmp/synthetic".into()],
        default_quality_profile: QualityProfileId::new(),
    };
    db.config().upsert_library(&library).await.unwrap();

    let content_id = ContentId::new();
    let node = cellarr_core::ContentNode {
        tags: Vec::new(),
        id: content_id,
        library_id,
        media_type: MediaType::Movie,
        parent_id: None,
        kind: cellarr_core::ContentKind::Movie,
        series_type: cellarr_core::SeriesType::Standard,
        coords: Coordinates::Movie,
        monitored: true,
        title_id: None,
    };
    use cellarr_core::repo::ContentRepository;
    db.content().upsert(&node).await.unwrap();
    ContentRef::new(content_id, library_id, MediaType::Movie, Coordinates::Movie).unwrap()
}

fn movie_registry(node: &ContentRef) -> MediaRegistry {
    let candidate = ContentCandidate {
        content_ref: node.clone(),
        title: "The Matrix".into(),
        aliases: Vec::new(),
    };
    let mut registry = MediaRegistry::new();
    registry.register(MovieModule::new(
        MockContentLookup { candidate },
        MockMetadata {
            movie: Some(MovieMeta {
                title: "The Matrix".into(),
                aliases: Vec::new(),
                year: Some(1999),
                external_ids: Vec::new(),
            }),
        },
    ));
    registry
}

#[allow(clippy::too_many_arguments)]
fn runner_config(
    library_root: PathBuf,
    client_host: String,
    mappings: Vec<RemotePathMapping>,
) -> RunnerConfig {
    RunnerConfig {
        content_tag_ids: Vec::new(),
        profile: permissive_profile(),
        custom_formats: Vec::<CustomFormat>::new(),
        ranking: QualityRanking::default(),
        proper_repack_policy: ProperRepackPolicy::default(),
        library_root,
        naming_format: "{Movie Title} ({Release Year})/{Movie Title}.{Extension}".into(),
        anime_naming_format: String::new(),
        series_type: cellarr_core::SeriesType::Standard,
        indexer_id: cellarr_core::IndexerId::new(),
        client_id: cellarr_core::DownloadClientId::new(),
        category: "cellarr".into(),
        max_track_polls: 5,
        track_poll_interval: std::time::Duration::ZERO,
        client_host,
        remote_path_mappings: mappings,
        write_nfo: false,
        delay_profiles: Vec::new(),
        release_profiles: Vec::new(),
        content_tags: Vec::new(),
        permissions: Default::default(),
        extra_files: Default::default(),
        indexer_criteria: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// 1) Blackhole watch-folder handoff through the real runner.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn blackhole_completed_file_drives_track_to_import() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();

    let watch = tmp.path().join("watch");
    let completed = tmp.path().join("completed");
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();

    let title = "The.Matrix.1999.1080p.BluRay.x264-GROUP";
    // The blackhole derives the download id (and thus the completed-file name)
    // from the release title stem, so the "external client" output is named after
    // it. We pre-place it so the first Track poll sees Completed.
    std::fs::create_dir_all(&completed).unwrap();
    let finished = completed.join(format!("{title}.mkv"));
    std::fs::write(&finished, b"synthetic movie bytes").unwrap();

    let node = seed_movie_node(&db).await;
    let registry = movie_registry(&node);
    let indexer = FakeIndexer {
        releases: vec![movie_release(title, "magnet:?xt=urn:btih:matrix")],
    };
    let client = BlackholeClient::with_transport(
        "blackhole",
        BlackholeSettings {
            watch_folder: watch.to_string_lossy().into_owned(),
            completed_folder: completed.to_string_lossy().into_owned(),
        },
        "cellarr",
        Protocol::Torrent,
        Box::new(cellarr_download::ReqwestTransport::new()),
    );
    let clock = LogicalClock::new(0);
    let config = runner_config(library_root.clone(), String::new(), Vec::new());

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
    let outcome = runner.run(&node).await.unwrap();

    let destinations = match outcome {
        RunOutcome::Imported { destinations, .. } => destinations,
        other => panic!("expected Imported via blackhole, got {other:?}"),
    };
    assert_eq!(destinations.len(), 1);
    let dest = PathBuf::from(&destinations[0]);
    assert!(dest.exists(), "imported file must exist at {dest:?}");
    assert!(dest.starts_with(&library_root));

    // The magnet job was written into the watch dir for the external client.
    assert!(watch.join(format!("{title}.magnet")).exists());
}

// ---------------------------------------------------------------------------
// 2) Remote-path mapping rewrite before Import.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mapped_remote_path_is_rewritten_before_import() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();

    // The file actually lives under the LOCAL prefix cellarr can see.
    let local_dir = tmp.path().join("data/downloads/movie");
    std::fs::create_dir_all(&local_dir).unwrap();
    let local_file = local_dir.join("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv");
    std::fs::write(&local_file, b"bytes").unwrap();

    // The client reports the path under the REMOTE prefix (its own mount).
    let reported = "/remote/downloads/movie/The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv";

    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_movie_node(&db).await;
    let registry = movie_registry(&node);
    let indexer = FakeIndexer {
        releases: vec![movie_release(
            "The.Matrix.1999.1080p.BluRay.x264-GROUP",
            "magnet:?xt=urn:btih:m",
        )],
    };
    let client = ReportingClient {
        reported_path: reported.to_string(),
    };
    let clock = LogicalClock::new(0);

    // Map /remote/downloads -> <tmp>/data/downloads so the reported path resolves.
    let mapping = RemotePathMapping {
        id: "m1".into(),
        host: String::new(),
        remote_path: "/remote/downloads".into(),
        local_path: tmp
            .path()
            .join("data/downloads")
            .to_string_lossy()
            .into_owned(),
    };
    let config = runner_config(library_root.clone(), String::new(), vec![mapping]);

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
    let outcome = runner.run(&node).await.unwrap();

    let destinations = match outcome {
        RunOutcome::Imported { destinations, .. } => destinations,
        other => panic!("expected Imported after remap, got {other:?}"),
    };
    assert_eq!(destinations.len(), 1);
    assert!(PathBuf::from(&destinations[0]).exists());
}

#[tokio::test]
async fn unmapped_remote_path_passes_through_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();

    // Reported under a prefix NO mapping covers; the path does not exist locally,
    // so Import holds for review — proving the path was NOT rewritten.
    let reported = "/unmapped/downloads/The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv";

    let node = seed_movie_node(&db).await;
    let registry = movie_registry(&node);
    let indexer = FakeIndexer {
        releases: vec![movie_release(
            "The.Matrix.1999.1080p.BluRay.x264-GROUP",
            "magnet:?xt=urn:btih:m",
        )],
    };
    let client = ReportingClient {
        reported_path: reported.to_string(),
    };
    let clock = LogicalClock::new(0);
    // A mapping exists but for a DIFFERENT prefix; the reported path must pass
    // through unchanged.
    let mapping = RemotePathMapping {
        id: "m1".into(),
        host: String::new(),
        remote_path: "/remote/downloads".into(),
        local_path: tmp.path().join("data").to_string_lossy().into_owned(),
    };
    let config = runner_config(library_root, String::new(), vec![mapping]);

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
    let outcome = runner.run(&node).await.unwrap();

    match outcome {
        RunOutcome::HeldForReview { reason } => {
            // The unchanged, non-existent path is what Import tried to read.
            assert!(
                reason.contains("/unmapped/downloads"),
                "held reason should reference the un-rewritten path, got: {reason}"
            );
        }
        other => panic!("expected HeldForReview for unmapped path, got {other:?}"),
    }
}
