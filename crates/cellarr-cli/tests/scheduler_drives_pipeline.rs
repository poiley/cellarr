//! The seam test the whole task exists for: prove the **scheduler → handler →
//! runner → import** chain is actually wired.
//!
//! Before this work the daemon's scheduler ran `cellarr-api`'s event-only
//! `CommandHandler`, which published a domain event and did NO search/grab/import
//! — nothing connected a fired job to the [`PipelineRunner`]. Here we assemble the
//! real [`LivePipelineHandler`] over a FAKE [`PipelineEnv`] (a fake indexer + a
//! fake download client + a temp library root), put it behind a real
//! [`Scheduler`], seed a monitored-missing movie, **submit a `MissingItemSearch`
//! and `tick` the scheduler**, and assert the file landed on disk in the library.
//!
//! No Docker, no network: the fake indexer returns a canned release and the fake
//! client "completes" with a real temp file. If the scheduler did not drive the
//! handler, or the handler did not drive the runner, no file would be imported and
//! the assertion fails — exactly the gap this closes.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use cellarr_cli::pipeline::{LivePipelineHandler, PipelineEnv};
use cellarr_core::repo::GrabRepository;
use cellarr_core::{
    ContentId, ContentKind, ContentNode, ContentRef, Coordinates, CustomFormat, DownloadClientId,
    DownloadState, DownloadStatus, GrabRequest, GrabStatus, Indexer, IndexerId, Library, LibraryId,
    MediaType, Protocol, QualityProfile, QualityProfileId, QualityRanking, Release, SearchTerms,
};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_jobs::runner::RunnerConfig;
use cellarr_jobs::{
    ConcurrencyCaps, JobKind, JobState, JobStore, MemoryJobStore, RetryPolicy, Scheduler,
    SystemClock,
};
use cellarr_media::{
    ContentCandidate, ContentLookup, MediaRegistry, MetadataLookup, MovieMeta, MovieModule,
    SeriesMeta,
};

// ---------------------------------------------------------------------------
// Fakes: an indexer that returns one release, a client that completes instantly.
// ---------------------------------------------------------------------------

/// A FAKE indexer that returns one canned movie release for every search.
struct FakeIndexer {
    release: Release,
}

#[derive(Debug, thiserror::Error)]
#[error("fake indexer error")]
struct FakeIndexerError;

#[async_trait]
impl Indexer for FakeIndexer {
    type Error = FakeIndexerError;
    fn name(&self) -> &str {
        "fake-indexer"
    }
    async fn search(&self, _terms: &SearchTerms) -> Result<Vec<Release>, Self::Error> {
        Ok(vec![self.release.clone()])
    }
    async fn latest(&self) -> Result<Vec<Release>, Self::Error> {
        Ok(vec![self.release.clone()])
    }
}

/// A FAKE download client that immediately completes with a real temp file.
struct FakeClient {
    content_path: String,
}

#[derive(Debug, thiserror::Error)]
#[error("fake client error")]
struct FakeClientError;

#[async_trait]
impl cellarr_core::DownloadClient for FakeClient {
    type Error = FakeClientError;
    fn name(&self) -> &str {
        "fake-client"
    }
    async fn add(&self, _grab: &GrabRequest) -> Result<String, Self::Error> {
        Ok("dl-1".into())
    }
    async fn status(&self, _id: &str) -> Result<DownloadStatus, Self::Error> {
        Ok(DownloadStatus {
            state: DownloadState::Completed,
            progress: 1.0,
            content_path: Some(self.content_path.clone()),
            ratio: Some(1.0),
            seeding_time_secs: Some(0),
            peers: Some(10),
            error_string: None,
        })
    }
    async fn remove(&self, _id: &str, _delete: bool) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// A FAKE [`PipelineEnv`] handing the handler the fake seams + a temp-rooted
/// runner config. This is the daemon's `LivePipelineEnv` substitute: same
/// contract, no DB-config/HTTP dependency.
struct FakeEnv {
    content_path: String,
    library_root: PathBuf,
    profile: QualityProfile,
}

#[async_trait]
impl PipelineEnv for FakeEnv {
    type Indexer = FakeIndexer;
    type Client = FakeClient;

    async fn resolve(
        &self,
        _content: &ContentRef,
    ) -> Result<Option<(Self::Indexer, Self::Client, RunnerConfig)>, String> {
        let release = Release {
            indexer_id: IndexerId::new(),
            title: "The.Matrix.1999.1080p.BluRay.x264-GROUP".into(),
            download_url: "magnet:?xt=urn:btih:abc".into(),
            guid: Some("the-matrix-1999".into()),
            protocol: Protocol::Torrent,
            size: Some(8_000_000_000),
            seeders: Some(100),
            indexer_flags: Vec::new(),
        };
        let config = RunnerConfig {
            profile: self.profile.clone(),
            custom_formats: Vec::<CustomFormat>::new(),
            ranking: QualityRanking::default(),
            proper_repack_policy: ProperRepackPolicy::default(),
            library_root: self.library_root.clone(),
            naming_format: "{Movie Title} ({Release Year})/{Movie Title}.{Extension}".into(),
            indexer_id: IndexerId::new(),
            client_id: DownloadClientId::new(),
            category: "cellarr".into(),
            max_track_polls: 5,
            track_poll_interval: std::time::Duration::ZERO,
            client_host: String::new(),
            remote_path_mappings: Vec::new(),
            write_nfo: false,
            delay_profiles: Vec::new(),
            content_tags: Vec::new(),
            permissions: Default::default(),
            extra_files: Default::default(),
            indexer_criteria: Default::default(),
        };
        Ok(Some((
            FakeIndexer { release },
            FakeClient {
                content_path: self.content_path.clone(),
            },
            config,
        )))
    }
}

// ---------------------------------------------------------------------------
// Media registry: a Movie module that resolves the seeded node.
// ---------------------------------------------------------------------------

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
    movie: MovieMeta,
}

#[async_trait]
impl MetadataLookup for MockMetadata {
    type Error = MockLookupError;
    async fn movie_meta(
        &self,
        _content: ContentId,
        _title_id: Option<cellarr_core::TitleId>,
    ) -> Result<Option<MovieMeta>, Self::Error> {
        Ok(Some(self.movie.clone()))
    }
    async fn series_meta(
        &self,
        _content: ContentId,
        _title_id: Option<cellarr_core::TitleId>,
    ) -> Result<Option<SeriesMeta>, Self::Error> {
        Ok(None)
    }
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
            movie: MovieMeta {
                title: "The Matrix".into(),
                aliases: Vec::new(),
                year: Some(1999),
                external_ids: Vec::new(),
            },
        },
    ));
    registry
}

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

/// Seed a monitored, file-less movie node (so `monitored_missing` returns it).
async fn seed_monitored_movie(db: &Database) -> ContentRef {
    let library_id = LibraryId::new();
    let profile = permissive_profile();
    db.profiles().upsert_profile(&profile).await.unwrap();
    let library = Library {
        id: library_id,
        media_type: MediaType::Movie,
        name: "Movies".into(),
        root_folders: vec!["/unused/in/this/test".into()],
        default_quality_profile: profile.id,
    };
    db.config().upsert_library(&library).await.unwrap();

    let content_id = ContentId::new();
    let node = ContentNode {
        id: content_id,
        library_id,
        media_type: MediaType::Movie,
        parent_id: None,
        kind: ContentKind::Movie,
        coords: Coordinates::Movie,
        monitored: true,
        title_id: None,
    };
    use cellarr_core::repo::ContentRepository;
    ContentRepository::upsert(&db.content(), &node)
        .await
        .unwrap();
    ContentRef::new(content_id, library_id, MediaType::Movie, Coordinates::Movie).unwrap()
}

// ---------------------------------------------------------------------------
// The test.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_item_search_through_scheduler_imports_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();

    // A real "downloaded" file the fake client points at on completion.
    let download_dir = tmp.path().join("downloads");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv");
    std::fs::write(&downloaded, b"synthetic movie bytes").unwrap();

    let library_root = tmp.path().join("library/movies");
    std::fs::create_dir_all(&library_root).unwrap();

    // Seed the monitored-missing movie + its registry.
    let node = seed_monitored_movie(&db).await;
    let registry = Arc::new(movie_registry(&node));

    // Assemble the REAL handler over the fake environment, behind a REAL scheduler.
    let events = cellarr_api::events::EventBus::default();
    let mut rx = events.subscribe();
    let env = FakeEnv {
        content_path: downloaded.to_string_lossy().into_owned(),
        library_root: library_root.clone(),
        profile: permissive_profile(),
    };
    let handler = Arc::new(LivePipelineHandler::new(
        db.clone(),
        registry,
        events.clone(),
        env,
    ));
    let scheduler = Scheduler::new(
        Arc::new(SystemClock),
        Arc::new(MemoryJobStore::new()),
        handler,
        ConcurrencyCaps::default(),
    );

    // Fire a MissingItemSearch through the scheduler and drive one tick.
    let job_id = scheduler
        .submit_now(JobKind::MissingItemSearch, RetryPolicy::default())
        .await
        .unwrap();
    let dispatched = scheduler.tick().await.unwrap();
    assert_eq!(dispatched, 1, "the submitted job was dispatched this tick");

    // The job completed (the chain ran to a terminal outcome).
    let job = scheduler.store().get(&job_id).await.unwrap().unwrap();
    assert_eq!(job.state, JobState::Done, "the MissingItemSearch succeeded");

    // THE PROOF: a real file was imported on disk under the library root.
    let imported = find_one_file(&library_root);
    let imported = imported.expect("an imported file must exist under the library root");
    assert!(imported.starts_with(&library_root));
    assert_eq!(
        std::fs::read(&imported).unwrap(),
        b"synthetic movie bytes",
        "the imported file is the downloaded content"
    );
    // The destination was rendered from the naming format + movie tokens.
    let rel = imported.strip_prefix(&library_root).unwrap();
    assert!(
        rel.to_string_lossy().contains("The Matrix"),
        "named from the movie title: {rel:?}"
    );

    // The grab row records the terminal Imported status.
    let history = cellarr_core::repo::HistoryRepository::for_content(&db.history(), node.id)
        .await
        .unwrap();
    let imported_grab = history.iter().find_map(|h| match h.event {
        cellarr_core::history::HistoryEvent::Imported { grab_id } => Some(grab_id),
        _ => None,
    });
    let imported_grab = imported_grab.expect("history records the import");
    let grab = GrabRepository::get(&db.grabs(), imported_grab)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grab.status, GrabStatus::Imported);

    // The handler published the live import event onto the shared bus.
    let mut saw_import = false;
    while let Ok(evt) = rx.try_recv() {
        if let cellarr_api::events::DomainEvent::ImportCompleted { content_id, .. } = evt {
            if content_id == node.id.to_string() {
                saw_import = true;
            }
        }
    }
    assert!(saw_import, "an ImportCompleted event was published");
}

/// Find exactly one regular file under `root` (recursively). The library layout is
/// `<root>/<Movie> (<Year>)/<Movie>.mkv`, so we walk for the leaf.
fn find_one_file(root: &std::path::Path) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}
