//! The anime absolute→episode remap, proven through the **REAL daemon wiring**:
//! `LivePipelineHandler` (with a scene provider attached via its
//! [`with_scene_provider`] builder — the same call boot makes) → runner →
//! Identify remap → import.
//!
//! This is the live counterpart to `cellarr-jobs/tests/anime_remap_pipeline.rs`
//! (which injects a provider straight onto a `PipelineRunner`). Here the provider
//! flows through the handler the daemon actually constructs, so it proves the
//! formerly-dead production call-site is wired: a fired job's Identify stage runs
//! the remap for an **anime-typed** series.
//!
//! Three cases, no Docker / no network (a fake indexer + fake client + a MOCK
//! scene provider over the REAL cellarr-db):
//!   1. an anime-typed series + an absolute release imports at the REMAPPED
//!      season/episode (absolute 13 → S02E01) and stores the absolute number;
//!   2. a NON-anime (standard) series is NOT remapped — the absolute coordinate is
//!      left untouched, so a wrong-episode import never happens (the gate);
//!   3. an anime-typed series whose absolute no mapping covers is HELD for manual
//!      resolution — never guessed onto a wrong episode (library-safety).
//!
//! [`with_scene_provider`]: cellarr_cli::pipeline::LivePipelineHandler::with_scene_provider

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use cellarr_cli::pipeline::{LivePipelineHandler, PipelineEnv};
use cellarr_core::{
    repo::ContentRepository, ContentId, ContentKind, ContentNode, ContentRef, Coordinates,
    CustomFormat, DownloadClientId, DownloadState, DownloadStatus, GrabRequest, Indexer, IndexerId,
    Library, LibraryId, MediaType, Protocol, QualityProfile, QualityProfileId, QualityRanking,
    Release, SearchTerms, SeriesType, TitleId,
};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_jobs::runner::RunnerConfig;
use cellarr_jobs::{
    ConcurrencyCaps, JobKind, JobState, JobStore, MemoryJobStore, RetryPolicy, Scheduler,
    SystemClock,
};
use cellarr_media::{
    ContentCandidate, ContentLookup, DynSceneMappingProvider, MediaRegistry, MetadataLookup,
    MovieMeta, SceneMapping, SceneRange, SeriesMeta, TvModule,
};

// --- fakes -----------------------------------------------------------------

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
/// config — the daemon's `LivePipelineEnv` substitute (same contract, no
/// DB-config/HTTP dependency). The release title carries the absolute number.
struct FakeEnv {
    release_title: String,
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
            title: self.release_title.clone(),
            download_url: "magnet:?xt=urn:btih:anime".into(),
            guid: Some("anime-release".into()),
            protocol: Protocol::Torrent,
            size: Some(1_000_000_000),
            seeders: Some(50),
            indexer_flags: Vec::new(),
        };
        let config = RunnerConfig {
            content_tag_ids: Vec::new(),
            profile: self.profile.clone(),
            custom_formats: Vec::<CustomFormat>::new(),
            ranking: QualityRanking::default(),
            proper_repack_policy: ProperRepackPolicy::default(),
            library_root: self.library_root.clone(),
            naming_format: "{Series Title}/S{Season}E{Episode}.{Extension}".into(),
            anime_naming_format: String::new(),
            series_type: cellarr_core::SeriesType::Standard,
            indexer_id: IndexerId::new(),
            client_id: DownloadClientId::new(),
            category: "cellarr".into(),
            max_track_polls: 5,
            stall_grace_polls: 0,
            track_poll_interval: std::time::Duration::ZERO,
            client_host: String::new(),
            remote_path_mappings: Vec::new(),
            write_nfo: false,
            delay_profiles: Vec::new(),
            release_profiles: Vec::new(),
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

// --- media registry --------------------------------------------------------

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

struct MockMetadata;
#[async_trait]
impl MetadataLookup for MockMetadata {
    type Error = MockLookupError;
    async fn movie_meta(
        &self,
        _content: ContentId,
        _title_id: Option<TitleId>,
    ) -> Result<Option<MovieMeta>, Self::Error> {
        Ok(None)
    }
    async fn series_meta(
        &self,
        _content: ContentId,
        _title_id: Option<TitleId>,
    ) -> Result<Option<SeriesMeta>, Self::Error> {
        Ok(Some(SeriesMeta {
            title: "The Show".into(),
            aliases: Vec::new(),
            year: Some(2018),
            external_ids: Vec::new(),
        }))
    }
}

fn tv_registry(node: &ContentRef) -> MediaRegistry {
    let candidate = ContentCandidate {
        content_ref: node.clone(),
        title: "The Show".into(),
        aliases: Vec::new(),
    };
    let mut registry = MediaRegistry::new();
    registry.register(TvModule::new(MockContentLookup { candidate }, MockMetadata));
    registry
}

/// The TVDB id the seeded series carries; the scene provider is keyed by it.
const TVDB_ID: i64 = 246_521;

/// A mock scene-mapping provider keyed by the series external id — mirrors how the
/// live `TvdbSceneMappings` answers, so the handler's `with_scene_provider` wiring
/// is exercised exactly as in production.
struct MockSceneProvider;
#[derive(Debug, thiserror::Error)]
#[error("mock scene provider error")]
struct MockSceneError;
#[async_trait]
impl cellarr_media::SceneMappingProvider for MockSceneProvider {
    type Error = MockSceneError;
    async fn scene_mapping(
        &self,
        series_external_id: &str,
    ) -> Result<Option<SceneMapping>, Self::Error> {
        if series_external_id == TVDB_ID.to_string() {
            // Two cours: S1 = absolute 1..=12, S2 = absolute 13..=24.
            Ok(Some(SceneMapping {
                series: TVDB_ID.to_string(),
                ranges: vec![
                    SceneRange {
                        season: 1,
                        start_absolute: 1,
                        length: 12,
                    },
                    SceneRange {
                        season: 2,
                        start_absolute: 13,
                        length: 12,
                    },
                ],
            }))
        } else {
            Ok(None)
        }
    }
}

fn scene_provider() -> Arc<dyn DynSceneMappingProvider> {
    Arc::new(MockSceneProvider)
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
        cutoff_quality: 26,
        min_custom_format_score: 0,
        upgrade_until_custom_format_score: 100,
        required_languages: Vec::new(),
    }
}

/// Seed a TV library, a series root (identity-linked to a `series_meta` row
/// carrying `TVDB_ID`, with the given [`SeriesType`]), and one episode node under
/// it at `(season, episode)`. Returns the episode [`ContentRef`] (the run target).
async fn seed_series(
    db: &Database,
    series_type: SeriesType,
    season: u32,
    episode: u32,
) -> ContentRef {
    let library_id = LibraryId::new();
    let profile = permissive_profile();
    db.profiles().upsert_profile(&profile).await.unwrap();
    let library = Library {
        id: library_id,
        media_type: MediaType::Tv,
        name: "Anime".into(),
        root_folders: vec!["/unused".into()],
        default_quality_profile: profile.id,
    };
    db.config().upsert_library(&library).await.unwrap();

    let series_id = ContentId::new();
    db.content()
        .upsert(&ContentNode {
            tags: Vec::new(),
            id: series_id,
            library_id,
            media_type: MediaType::Tv,
            parent_id: None,
            kind: ContentKind::Series,
            series_type,
            coords: Coordinates::Episode {
                season: 0,
                episode: 0,
                absolute: None,
            },
            monitored: true,
            title_id: None,
        })
        .await
        .unwrap();
    // Link the series' TVDB id the way the identify pipeline does (writes the
    // `series_meta` identity row the live `series_tvdb_id` query reads). This is
    // what the scene provider is keyed by.
    db.content()
        .link_external_id(
            series_id,
            MediaType::Tv,
            "tvdb",
            &TVDB_ID.to_string(),
            "The Show",
        )
        .await
        .unwrap();

    let episode_id = ContentId::new();
    let coords = Coordinates::Episode {
        season,
        episode,
        absolute: None,
    };
    db.content()
        .upsert(&ContentNode {
            tags: Vec::new(),
            id: episode_id,
            library_id,
            media_type: MediaType::Tv,
            parent_id: Some(series_id),
            kind: ContentKind::Episode,
            series_type,
            coords: coords.clone(),
            monitored: true,
            title_id: None,
        })
        .await
        .unwrap();

    ContentRef::new(episode_id, library_id, MediaType::Tv, coords).unwrap()
}

/// Assemble the REAL handler over the fake env, with the scene provider attached
/// the way boot does, behind a REAL scheduler. Returns the scheduler.
fn build_scheduler(
    db: &Database,
    registry: Arc<MediaRegistry>,
    env: FakeEnv,
    with_scene: bool,
) -> Scheduler<SystemClock, MemoryJobStore, LivePipelineHandler<FakeEnv>> {
    let events = cellarr_api::events::EventBus::default();
    let mut handler = LivePipelineHandler::new(db.clone(), registry, events, env);
    if with_scene {
        handler = handler.with_scene_provider(scene_provider());
    }
    Scheduler::new(
        Arc::new(SystemClock),
        Arc::new(MemoryJobStore::new()),
        Arc::new(handler),
        ConcurrencyCaps::default(),
    )
}

fn find_one_file(root: &Path) -> Option<PathBuf> {
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

/// Run one `ManualSearch` for `node` through the scheduler, returning the job
/// state after the tick.
async fn run_manual_search(
    scheduler: &Scheduler<SystemClock, MemoryJobStore, LivePipelineHandler<FakeEnv>>,
    node: &ContentRef,
) -> JobState {
    let job_id = scheduler
        .submit_now(
            JobKind::ManualSearch {
                content_id: node.id.to_string(),
            },
            RetryPolicy::default(),
        )
        .await
        .unwrap();
    let dispatched = scheduler.tick().await.unwrap();
    assert_eq!(dispatched, 1, "the submitted job dispatched");
    // The scheduler spawns jobs; await the run before reading its terminal state.
    scheduler.join_in_flight().await;
    scheduler.store().get(&job_id).await.unwrap().unwrap().state
}

// --- tests -----------------------------------------------------------------

/// An anime-typed series + an absolute release imports at the REMAPPED
/// season/episode through the live handler wiring (absolute 13 → S02E01), and the
/// reconciled absolute number is stored on the imported episode's media file's
/// coordinate path (the file lands at S02E01, proving the remap drove naming).
#[tokio::test]
async fn anime_series_absolute_release_remaps_through_live_handler() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();

    let download_dir = tmp.path().join("downloads/anime");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("The.Show.S02E01.1080p.mkv");
    std::fs::write(&downloaded, b"synthetic anime bytes").unwrap();

    let library_root = tmp.path().join("library/anime");
    std::fs::create_dir_all(&library_root).unwrap();

    // Absolute 13 must land on the seeded S02E01 episode node.
    let node = seed_series(&db, SeriesType::Anime, 2, 1).await;
    let registry = Arc::new(tv_registry(&node));
    let env = FakeEnv {
        release_title: "[SubsPlease] The Show - 13 (1080p) [ABCD1234].mkv".into(),
        content_path: downloaded.to_string_lossy().into_owned(),
        library_root: library_root.clone(),
        profile: permissive_profile(),
    };
    let scheduler = build_scheduler(&db, registry, env, /* with_scene = */ true);

    let state = run_manual_search(&scheduler, &node).await;
    assert_eq!(state, JobState::Done, "the anime ManualSearch succeeded");

    // The search grabs (against the REMAPPED S02E01 node) and defers tracking; the
    // ReconcileDownloads job finalizes the import. Drive it so the file lands.
    scheduler
        .submit_now(JobKind::ReconcileDownloads, RetryPolicy::default())
        .await
        .unwrap();
    scheduler.tick().await.unwrap();
    scheduler.join_in_flight().await;

    // THE PROOF: a real file landed under the library at the REMAPPED S02E01.
    let imported = find_one_file(&library_root).expect("an imported file under the library root");
    assert!(imported.starts_with(&library_root));
    assert_eq!(
        imported.file_name().unwrap().to_str().unwrap(),
        "S02E01.mkv",
        "the file lands at the remapped S02E01, not the absolute number"
    );
    assert_eq!(
        std::fs::read(&imported).unwrap(),
        b"synthetic anime bytes",
        "the imported file is the downloaded content"
    );

    // The reconciled absolute number is persisted on the imported episode: a
    // media_file is linked to the node (so it is no longer "missing").
    use cellarr_core::repo::MediaFileRepository;
    let files = db.media_files().list_for_content(node.id).await.unwrap();
    assert_eq!(
        files.len(),
        1,
        "the imported file is linked to the episode node"
    );
}

/// A NON-anime (standard) series is NOT force-remapped: the absolute coordinate is
/// left untouched (the series-type gate is closed), so the release does not
/// confidently identify to the S02E01 node and NOTHING is imported onto the wrong
/// episode. The gate keeps a standard show safe.
#[tokio::test]
async fn standard_series_absolute_release_is_not_remapped() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();

    let download_dir = tmp.path().join("downloads/std");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("The.Show.S02E01.1080p.mkv");
    std::fs::write(&downloaded, b"synthetic bytes").unwrap();

    let library_root = tmp.path().join("library/std");
    std::fs::create_dir_all(&library_root).unwrap();

    // A STANDARD series: absolute 13 must NOT be remapped onto this S02E01 node.
    let node = seed_series(&db, SeriesType::Standard, 2, 1).await;
    let registry = Arc::new(tv_registry(&node));
    let env = FakeEnv {
        release_title: "[SubsPlease] The Show - 13 (1080p) [ABCD1234].mkv".into(),
        content_path: downloaded.to_string_lossy().into_owned(),
        library_root: library_root.clone(),
        profile: permissive_profile(),
    };
    // A scene provider IS attached (mirrors prod): the GATE — not the absence of a
    // provider — is what stops the remap for a standard series.
    let scheduler = build_scheduler(&db, registry, env, /* with_scene = */ true);

    let state = run_manual_search(&scheduler, &node).await;
    // The job completes (a non-import outcome is a normal, logged result, not a
    // failure of the chain).
    assert_eq!(state, JobState::Done);

    // THE PROOF: nothing was imported onto the wrong episode — the un-remapped
    // absolute did not confidently match the S02E01 node, so no file landed.
    assert!(
        find_one_file(&library_root).is_none(),
        "a standard series must not import an absolute release onto a guessed episode"
    );
    use cellarr_core::repo::MediaFileRepository;
    assert!(
        db.media_files()
            .list_for_content(node.id)
            .await
            .unwrap()
            .is_empty(),
        "no media file is linked to the standard-series episode node"
    );
}

/// An anime-typed series whose absolute number no scene mapping covers is HELD for
/// manual resolution — never guessed onto a wrong episode. The run holds; nothing
/// is imported. (Library-safety, through the live handler.)
#[tokio::test]
async fn anime_unmapped_absolute_is_held_not_guessed_through_live_handler() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();

    let library_root = tmp.path().join("library/anime");
    std::fs::create_dir_all(&library_root).unwrap();

    // The mapping covers absolutes 1..=24; 99 is out of range.
    let node = seed_series(&db, SeriesType::Anime, 2, 1).await;
    let registry = Arc::new(tv_registry(&node));
    let env = FakeEnv {
        release_title: "[SubsPlease] The Show - 99 (1080p) [ABCD1234].mkv".into(),
        content_path: "/nonexistent".into(),
        library_root: library_root.clone(),
        profile: permissive_profile(),
    };
    let scheduler = build_scheduler(&db, registry, env, /* with_scene = */ true);

    let state = run_manual_search(&scheduler, &node).await;
    assert_eq!(state, JobState::Done);

    // Nothing was imported (held before Grab), and no grab row was created.
    assert!(
        find_one_file(&library_root).is_none(),
        "an unmapped absolute must not place any file in the library"
    );
    use cellarr_core::repo::GrabRepository;
    let grabs = db.grabs().list().await.unwrap();
    assert!(
        grabs.is_empty(),
        "an unmapped absolute must not create a grab"
    );
}
