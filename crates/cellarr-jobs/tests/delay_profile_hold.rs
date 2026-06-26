//! Delay-profile integration over the real pipeline runner.
//!
//! Drives a grabbable movie release through Discover→Decide with a delay profile
//! configured, and asserts that:
//!   * within the protocol's delay window the release is HELD (the run rejects
//!     with a "held by delay profile" reason and nothing is grabbed), and
//!   * once the window elapses (the logical clock is advanced past it) the same
//!     run grabs and imports the release.
//!
//! A second test proves `bypassIfHighestQuality` grabs a cutoff-quality release
//! immediately, never delaying it.
//!
//! The seams are the same offline fakes the centerpiece e2e uses (a canned
//! indexer + an immediately-completing client over temp files); only the delay
//! profile and the clock stepping are new here.

use std::path::PathBuf;

use async_trait::async_trait;

use cellarr_core::{
    ContentId, ContentRef, Coordinates, CustomFormat, DelayProfile, LibraryId, MediaType,
    PreferredProtocol, Protocol, QualityProfile, QualityProfileId, QualityRanking, Release,
    SearchTerms,
};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_jobs::clock::LogicalClock;
use cellarr_jobs::runner::{PipelineRunner, RunOutcome, RunnerConfig};
use cellarr_media::{
    ContentCandidate, ContentLookup, MediaRegistry, MetadataLookup, MovieMeta, MovieModule,
    SeriesMeta,
};

// --- offline seams ---------------------------------------------------------

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
        Ok(None)
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
        // A low cutoff so an ordinary 1080p Bluray release is "highest quality"
        // for the bypass test below.
        cutoff_quality: 14,
        min_custom_format_score: 0,
        upgrade_until_custom_format_score: 100,
        required_languages: Vec::new(),
    }
}

fn movie_release(title: &str, protocol: Protocol) -> Release {
    Release {
        indexer_id: cellarr_core::IndexerId::new(),
        title: title.to_string(),
        download_url: "magnet:?xt=synthetic".into(),
        guid: Some(format!("guid-{title}")),
        protocol,
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
        name: "movies".into(),
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
        coords: Coordinates::Movie,
        monitored: true,
        title_id: None,
    };
    use cellarr_core::repo::ContentRepository;
    db.content().upsert(&node).await.unwrap();
    ContentRef::new(content_id, library_id, MediaType::Movie, Coordinates::Movie).unwrap()
}

fn registry_for(node: &ContentRef) -> MediaRegistry {
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

fn runner_config(library_root: PathBuf, delay_profiles: Vec<DelayProfile>) -> RunnerConfig {
    RunnerConfig {
        content_tag_ids: Vec::new(),
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
        delay_profiles,
        release_profiles: Vec::new(),
        content_tags: Vec::new(),
        permissions: Default::default(),
        extra_files: Default::default(),
        indexer_criteria: Default::default(),
    }
}

/// A torrent delay profile holding for `minutes`, no preference, no bypass.
fn torrent_delay(minutes: u32) -> DelayProfile {
    DelayProfile {
        id: cellarr_core::DelayProfileId::new(),
        enabled: true,
        preferred_protocol: PreferredProtocol::Either,
        usenet_delay: 0,
        torrent_delay: minutes,
        bypass_if_highest_quality: false,
        tags: Vec::new(),
        order: 0,
    }
}

// --- tests -----------------------------------------------------------------

#[tokio::test]
async fn delay_profile_holds_release_until_window_elapses() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();

    let download_dir = tmp.path().join("downloads");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv");
    std::fs::write(&downloaded, b"bytes").unwrap();
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_movie_node(&db).await;
    let registry = registry_for(&node);
    let indexer = FakeIndexer {
        releases: vec![movie_release(
            "The.Matrix.1999.1080p.BluRay.x264-GROUP",
            Protocol::Torrent,
        )],
    };
    let client = FakeDownloadClient {
        content_path: downloaded.to_string_lossy().into_owned(),
    };

    // A 30-minute torrent delay; the clock starts at t=0.
    let clock = LogicalClock::new(0);
    let config = runner_config(library_root.clone(), vec![torrent_delay(30)]);
    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);

    // First run, within the window: the release is HELD, not grabbed.
    let outcome = runner.run(&node).await.unwrap();
    match outcome {
        RunOutcome::Rejected { reason } => {
            assert!(
                reason.contains("held by delay profile"),
                "expected a delay-hold reject, got: {reason}"
            );
        }
        other => panic!("expected the release to be held, got {other:?}"),
    }
    // The first-seen instant was recorded.
    let pending = db
        .pending_releases()
        .list_for_content(node.id)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1, "the held release is tracked as pending");
    assert_eq!(pending[0].first_seen_at, 0);

    // Advance past the 30-minute window (30 * 60 = 1800s) and re-run: now it grabs
    // and imports, and the pending row is cleared.
    clock.set(1800);
    let outcome2 = runner.run(&node).await.unwrap();
    assert!(
        matches!(outcome2, RunOutcome::Imported { .. }),
        "expected Imported after the delay elapsed, got {outcome2:?}"
    );
    let pending2 = db
        .pending_releases()
        .list_for_content(node.id)
        .await
        .unwrap();
    assert!(
        pending2.is_empty(),
        "the pending row is cleared once grabbed"
    );
}

/// A torrent delay profile scoped to the given case-insensitive label tags.
fn tagged_torrent_delay(minutes: u32, tags: Vec<String>) -> DelayProfile {
    DelayProfile {
        tags,
        ..torrent_delay(minutes)
    }
}

#[tokio::test]
async fn tag_scoped_delay_profile_holds_only_tagged_content() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let download_dir = tmp.path().join("downloads");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv");
    std::fs::write(&downloaded, b"bytes").unwrap();
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_movie_node(&db).await;
    let registry = registry_for(&node);
    let indexer = FakeIndexer {
        releases: vec![movie_release(
            "The.Matrix.1999.1080p.BluRay.x264-GROUP",
            Protocol::Torrent,
        )],
    };
    let client = FakeDownloadClient {
        content_path: downloaded.to_string_lossy().into_owned(),
    };

    // A 30-minute delay scoped to the "anime" tag. The content's resolved tag
    // labels are what the runner threads into `content_tags`.
    let profiles = vec![tagged_torrent_delay(30, vec!["anime".into()])];

    // Content tagged "Anime" (case-insensitive) -> the profile applies -> HELD.
    let clock = LogicalClock::new(0);
    let mut config = runner_config(library_root.clone(), profiles.clone());
    config.content_tags = vec!["Anime".into()];
    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
    let outcome = runner.run(&node).await.unwrap();
    assert!(
        matches!(outcome, RunOutcome::Rejected { ref reason } if reason.contains("held by delay profile")),
        "a tag-scoped profile holds content sharing its tag, got {outcome:?}"
    );

    // Clear the pending row so the second scenario starts clean.
    db.pending_releases()
        .clear(
            node.id,
            &movie_release("The.Matrix.1999.1080p.BluRay.x264-GROUP", Protocol::Torrent),
        )
        .await
        .unwrap();

    // The SAME tagged profile against UNtagged content does not apply (the
    // catch-all is absent), so resolve_delay_profile finds no governing profile
    // and the release grabs+imports immediately.
    let clock2 = LogicalClock::new(0);
    let config2 = runner_config(library_root.clone(), profiles);
    let runner2 = PipelineRunner::new(&indexer, &client, &registry, &db, &clock2, &config2);
    let outcome2 = runner2.run(&node).await.unwrap();
    assert!(
        matches!(outcome2, RunOutcome::Imported { .. }),
        "a tag-scoped profile never holds untagged content, got {outcome2:?}"
    );
}

#[tokio::test]
async fn bypass_if_highest_quality_grabs_immediately() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();

    let download_dir = tmp.path().join("downloads");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv");
    std::fs::write(&downloaded, b"bytes").unwrap();
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_movie_node(&db).await;
    let registry = registry_for(&node);
    let indexer = FakeIndexer {
        // A Bluray-1080p release: its quality rank meets the profile cutoff (14),
        // so the bypass treats it as the highest worth waiting for.
        releases: vec![movie_release(
            "The.Matrix.1999.1080p.BluRay.x264-GROUP",
            Protocol::Torrent,
        )],
    };
    let client = FakeDownloadClient {
        content_path: downloaded.to_string_lossy().into_owned(),
    };

    // A long torrent delay BUT bypass-on-highest-quality enabled.
    let mut profile = torrent_delay(120);
    profile.bypass_if_highest_quality = true;
    let clock = LogicalClock::new(0);
    let config = runner_config(library_root, vec![profile]);
    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);

    // Even at t=0 (deep inside the window) the highest-quality release is grabbed.
    let outcome = runner.run(&node).await.unwrap();
    assert!(
        matches!(outcome, RunOutcome::Imported { .. }),
        "highest-quality release must bypass the delay, got {outcome:?}"
    );
}
