//! The anime absolute->episode XEM remap, wired end-to-end through the REAL
//! pipeline runner.
//!
//! These prove the formerly-dead call-site is live: an absolute-numbered anime
//! release flows Discover -> Parse -> (absolute remap via the scene-mapping
//! provider, gated on the db identity-link query that resolves a content node ->
//! its series' TVDB id) -> Identify -> ... -> Imported, landing at the correct
//! season/episode. The negative test pins the library-safety rule: an absolute
//! number no mapping covers is surfaced for manual resolution, never guessed.
//!
//! Seams: a FAKE indexer + download client, a MOCK scene-mapping provider, a
//! MOCK content/metadata seam for the TV module, and the REAL cellarr-db
//! (tempfile SQLite) seeded with a series carrying a real TVDB id so the live
//! `series_tvdb_id` query has something to resolve.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use cellarr_core::{
    repo::ContentRepository, ContentId, ContentKind, ContentNode, ContentRef, Coordinates,
    CustomFormat, LibraryId, MediaType, Protocol, QualityProfile, QualityProfileId, QualityRanking,
    Release, SearchTerms, TitleId,
};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_jobs::clock::LogicalClock;
use cellarr_jobs::runner::{PipelineRunner, RunOutcome, RunnerConfig};
use cellarr_media::{
    ContentCandidate, ContentLookup, DynSceneMappingProvider, MediaRegistry, MetadataLookup,
    MovieMeta, SceneMapping, SceneRange, SeriesMeta, TvModule,
};

// --- synthetic seams (offline) ---------------------------------------------

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

struct FakeDownloadClient {
    content_path: String,
}
#[derive(Debug, thiserror::Error)]
#[error("fake download client error")]
struct FakeClientError;
#[async_trait]
impl cellarr_core::traits::DownloadClient for FakeDownloadClient {
    type Error = FakeClientError;
    fn name(&self) -> &str {
        "fake-client"
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

/// A mock content lookup returning one candidate (the episode node under test).
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
    series: Option<SeriesMeta>,
}
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
        Ok(self.series.clone())
    }
}

/// A mock scene-mapping provider keyed by external id, returning the fixture
/// mapping. Mirrors how the live TheXEM provider answers.
struct MockSceneProvider {
    external_id: String,
    mapping: SceneMapping,
}
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
        if series_external_id == self.external_id {
            Ok(Some(self.mapping.clone()))
        } else {
            Ok(None)
        }
    }
}

// --- fixtures --------------------------------------------------------------

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

fn anime_release(title: &str) -> Release {
    Release {
        indexer_id: cellarr_core::IndexerId::new(),
        title: title.to_string(),
        download_url: "magnet:?xt=synthetic-anime".into(),
        guid: Some("guid-anime".into()),
        protocol: Protocol::Torrent,
        size: Some(1_000_000_000),
        seeders: Some(20),
        indexer_flags: Vec::new(),
    }
}

fn runner_config(library_root: PathBuf) -> RunnerConfig {
    RunnerConfig {
        profile: permissive_profile(),
        custom_formats: Vec::<CustomFormat>::new(),
        ranking: QualityRanking::default(),
        proper_repack_policy: ProperRepackPolicy::default(),
        library_root,
        naming_format: "{Series Title}/S{Season}E{Episode}.{Extension}".to_string(),
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

/// The TVDB id the seeded series carries; the scene provider is keyed by it.
const TVDB_ID: i64 = 246_521;

/// Seed a TV library, a series root node identity-linked to a `series_meta` row
/// carrying `TVDB_ID`, and one episode node under it. Returns the episode
/// [`ContentRef`] (the acquisition target) and its season/episode.
async fn seed_anime_series(db: &Database, season: u32, episode: u32) -> ContentRef {
    let library_id = LibraryId::new();
    let library = cellarr_core::Library {
        id: library_id,
        media_type: MediaType::Tv,
        name: "Anime".into(),
        root_folders: vec!["/tmp/synthetic".into()],
        default_quality_profile: QualityProfileId::new(),
    };
    db.config().upsert_library(&library).await.unwrap();

    // The series identity row (this is what the `series_tvdb_id` query reads).
    let title_id = TitleId::new();
    sqlx::query("INSERT INTO series_meta (title_id, title, year, tvdb_id) VALUES (?1, ?2, ?3, ?4)")
        .bind(title_id.to_string())
        .bind("The Show")
        .bind(2018_i64)
        .bind(TVDB_ID)
        .execute(db.pool())
        .await
        .unwrap();

    // The series root node, identity-linked to that row.
    let series_id = ContentId::new();
    db.content()
        .upsert(&ContentNode {
            id: series_id,
            library_id,
            media_type: MediaType::Tv,
            parent_id: None,
            kind: ContentKind::Series,
            coords: Coordinates::Episode {
                season: 0,
                episode: 0,
                absolute: None,
            },
            monitored: true,
            title_id: Some(title_id),
        })
        .await
        .unwrap();

    // The episode node under the series (the run target).
    let episode_id = ContentId::new();
    let coords = Coordinates::Episode {
        season,
        episode,
        absolute: None,
    };
    db.content()
        .upsert(&ContentNode {
            id: episode_id,
            library_id,
            media_type: MediaType::Tv,
            parent_id: Some(series_id),
            kind: ContentKind::Episode,
            coords: coords.clone(),
            monitored: true,
            title_id: None,
        })
        .await
        .unwrap();

    ContentRef::new(episode_id, library_id, MediaType::Tv, coords).unwrap()
}

fn registry_for(node: &ContentRef) -> MediaRegistry {
    let candidate = ContentCandidate {
        content_ref: node.clone(),
        title: "The Show".to_string(),
        aliases: Vec::new(),
    };
    let mut registry = MediaRegistry::new();
    registry.register(TvModule::new(
        MockContentLookup { candidate },
        MockMetadata {
            series: Some(SeriesMeta {
                title: "The Show".into(),
                aliases: Vec::new(),
                year: Some(2018),
                external_ids: Vec::new(),
            }),
        },
    ));
    registry
}

/// Two cours: season 1 = absolute 1..=12, season 2 = absolute 13..=24.
fn mapping() -> SceneMapping {
    SceneMapping {
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
    }
}

fn scene_provider() -> Arc<dyn DynSceneMappingProvider> {
    Arc::new(MockSceneProvider {
        external_id: TVDB_ID.to_string(),
        mapping: mapping(),
    })
}

// --- tests -----------------------------------------------------------------

/// An absolute-numbered anime release identifies to the correct season/episode
/// through the REAL runner path: absolute 13 maps to S02E01, which is the seeded
/// episode node, and the file is imported.
#[tokio::test]
async fn absolute_anime_release_remaps_to_season_episode_through_the_runner() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();

    // The downloaded file is named at its canonical S02E01 so the second-parse
    // verification agrees with the remapped grab intent.
    let download_dir = tmp.path().join("downloads/anime");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("The.Show.S02E01.1080p.mkv");
    std::fs::write(&downloaded, b"synthetic anime bytes").unwrap();

    let library_root = tmp.path().join("library/anime");
    std::fs::create_dir_all(&library_root).unwrap();

    // Absolute 13 must land on season 2, episode 1.
    let node = seed_anime_series(&db, 2, 1).await;
    let registry = registry_for(&node);

    let indexer = FakeIndexer {
        releases: vec![anime_release(
            "[SubsPlease] The Show - 13 (1080p) [ABCD1234].mkv",
        )],
    };
    let client = FakeDownloadClient {
        content_path: downloaded.to_string_lossy().into_owned(),
    };
    let clock = LogicalClock::new(0);
    let config = runner_config(library_root.clone());

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config)
        .with_scene_provider(scene_provider());
    let outcome = runner.run(&node).await.unwrap();

    let destinations = match outcome {
        RunOutcome::Imported { destinations, .. } => destinations,
        other => panic!("absolute release should remap+import, got {other:?}"),
    };
    assert_eq!(destinations.len(), 1);
    let dest = PathBuf::from(&destinations[0]);
    assert!(dest.exists(), "imported file must exist at {dest:?}");
    // The file landed at the REMAPPED season/episode (S02E01), proving the
    // absolute->episode reconciliation drove naming.
    assert_eq!(
        dest.file_name().unwrap().to_str().unwrap(),
        "S02E01.mkv",
        "the file must land at the remapped S02E01, not the absolute number"
    );
}

/// An absolute number no scene mapping covers is surfaced for manual resolution —
/// never guessed onto a season/episode. The run holds; nothing is grabbed.
#[tokio::test]
async fn unmapped_absolute_is_surfaced_for_manual_resolution_not_guessed() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();

    let library_root = tmp.path().join("library/anime");
    std::fs::create_dir_all(&library_root).unwrap();

    // The mapping covers absolutes 1..=24; 99 is out of range.
    let node = seed_anime_series(&db, 2, 1).await;
    let registry = registry_for(&node);

    let indexer = FakeIndexer {
        releases: vec![anime_release(
            "[SubsPlease] The Show - 99 (1080p) [ABCD1234].mkv",
        )],
    };
    let client = FakeDownloadClient {
        content_path: "/nonexistent".into(),
    };
    let clock = LogicalClock::new(0);
    let config = runner_config(library_root.clone());

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config)
        .with_scene_provider(scene_provider());
    let outcome = runner.run(&node).await.unwrap();

    match &outcome {
        RunOutcome::HeldForReview { reason } => {
            assert!(
                reason.contains("manual resolution"),
                "an unmapped absolute must be surfaced for manual resolution, got: {reason}"
            );
        }
        other => panic!("unmapped absolute must be held, not {other:?} (never guessed)"),
    }

    // No file was moved into the library (nothing was guessed onto disk).
    let mut entries = std::fs::read_dir(&library_root).unwrap();
    assert!(
        entries.next().is_none(),
        "an unmapped absolute must not place any file in the library"
    );

    // No grab row was created (the run held before Grab).
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM grab")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count.0, 0, "an unmapped absolute must not create a grab");
}

/// A series with no linked TVDB id cannot resolve an absolute number, so the
/// absolute release is surfaced for manual resolution (the identity-link query
/// returns None) — never guessed.
#[tokio::test]
async fn absolute_without_linked_tvdb_id_is_surfaced_not_guessed() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();

    let library_root = tmp.path().join("library/anime");
    std::fs::create_dir_all(&library_root).unwrap();

    // Seed an episode node WITHOUT a series_meta / tvdb link.
    let library_id = LibraryId::new();
    db.config()
        .upsert_library(&cellarr_core::Library {
            id: library_id,
            media_type: MediaType::Tv,
            name: "Anime".into(),
            root_folders: vec!["/tmp/synthetic".into()],
            default_quality_profile: QualityProfileId::new(),
        })
        .await
        .unwrap();
    let episode_id = ContentId::new();
    let coords = Coordinates::Episode {
        season: 2,
        episode: 1,
        absolute: None,
    };
    db.content()
        .upsert(&ContentNode {
            id: episode_id,
            library_id,
            media_type: MediaType::Tv,
            parent_id: None,
            kind: ContentKind::Episode,
            coords: coords.clone(),
            monitored: true,
            title_id: None,
        })
        .await
        .unwrap();
    let node = ContentRef::new(episode_id, library_id, MediaType::Tv, coords).unwrap();
    let registry = registry_for(&node);

    let indexer = FakeIndexer {
        releases: vec![anime_release(
            "[SubsPlease] The Show - 13 (1080p) [ABCD1234].mkv",
        )],
    };
    let client = FakeDownloadClient {
        content_path: "/nonexistent".into(),
    };
    let clock = LogicalClock::new(0);
    let config = runner_config(library_root.clone());

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config)
        .with_scene_provider(scene_provider());
    let outcome = runner.run(&node).await.unwrap();

    match &outcome {
        RunOutcome::HeldForReview { reason } => assert!(
            reason.contains("manual resolution"),
            "unlinked series absolute must be surfaced, got: {reason}"
        ),
        other => panic!("expected HeldForReview, got {other:?}"),
    }
}
