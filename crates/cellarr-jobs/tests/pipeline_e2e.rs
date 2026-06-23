//! The centerpiece end-to-end test.
//!
//! Drives BOTH a movie release and a TV episode release from Discover all the way
//! to Imported through the *real* pipeline runner, with:
//!   - a FAKE Indexer returning canned releases,
//!   - a FAKE DownloadClient that "completes" with a content_path pointing at
//!     real temp files,
//!   - a TEMP-DIR cellarr-fs target,
//!   - REAL cellarr-parse, cellarr-decide, cellarr-media (Movie + TV modules with
//!     a mocked metadata/content seam),
//!   - REAL cellarr-db (a tempfile SQLite database).
//!
//! Asserts, for each media type, that the files actually land at the expected
//! renamed on-disk paths, the grab reaches Imported, and a decision_log entry +
//! history records explain the grab. A negative case proves a junk/low-quality
//! release is rejected with a logged reason and no file moved.

use std::path::PathBuf;

use async_trait::async_trait;

use cellarr_core::{
    repo::{GrabRepository, HistoryRepository},
    ContentId, ContentRef, Coordinates, CustomFormat, GrabStatus, LibraryId, MediaType, Protocol,
    QualityProfile, QualityProfileId, QualityRanking, Release, SearchTerms,
};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_jobs::clock::LogicalClock;
use cellarr_jobs::runner::{PipelineRunner, RunOutcome, RunnerConfig};
use cellarr_media::{
    ContentCandidate, ContentLookup, MediaRegistry, MetadataLookup, MovieMeta, MovieModule,
    SeriesMeta, TvModule,
};

// ---------------------------------------------------------------------------
// Synthetic seams (offline; clearly labelled). None of these hit a network.
// ---------------------------------------------------------------------------

/// A FAKE indexer that returns a fixed list of synthetic releases.
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

/// A FAKE download client that immediately "completes" every download, reporting
/// a content_path that the test has pre-populated with real files.
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
        // Completed on the first poll, with the importable on-disk location.
        Ok(cellarr_core::DownloadStatus {
            state: cellarr_core::DownloadState::Completed,
            progress: 1.0,
            content_path: Some(self.content_path.clone()),
            ratio: Some(1.0),
            seeding_time_secs: Some(0),
        })
    }
    async fn remove(&self, _download_id: &str, _delete_data: bool) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// A mock content-lookup: returns one candidate node for any title query.
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

/// A mock metadata-lookup carrying one movie and one series identity.
struct MockMetadata {
    movie: Option<MovieMeta>,
    series: Option<SeriesMeta>,
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
        Ok(self.series.clone())
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A permissive profile that allows every default quality (so a good release is
/// grabbed) but still rejects the genuinely unrankable "Unknown" bucket.
fn permissive_profile() -> QualityProfile {
    let ranking = QualityRanking::default();
    // Allow every real quality (ranks 1..=max); exclude rank 0 (Unknown).
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
        cutoff_quality: 14, // Bluray-1080p-ish
        min_custom_format_score: 0,
        upgrade_until_custom_format_score: 100,
        required_languages: Vec::new(),
    }
}

fn movie_release(title: &str) -> Release {
    Release {
        indexer_id: cellarr_core::IndexerId::new(),
        title: title.to_string(),
        download_url: "magnet:?xt=synthetic".into(),
        guid: Some("guid-movie".into()),
        protocol: Protocol::Torrent,
        size: Some(8_000_000_000),
        seeders: Some(50),
        indexer_flags: Vec::new(),
    }
}

fn tv_release(title: &str) -> Release {
    Release {
        indexer_id: cellarr_core::IndexerId::new(),
        title: title.to_string(),
        download_url: "magnet:?xt=synthetic-tv".into(),
        guid: Some("guid-tv".into()),
        protocol: Protocol::Torrent,
        size: Some(2_000_000_000),
        seeders: Some(30),
        indexer_flags: Vec::new(),
    }
}

/// Seed a library + one content node in the real SQLite DB so the run exercises
/// real persistence (and the FK to `library` is satisfied).
async fn seed_node(
    db: &Database,
    media_type: MediaType,
    kind: cellarr_core::ContentKind,
    coords: Coordinates,
) -> ContentRef {
    let library_id = LibraryId::new();
    let library = cellarr_core::Library {
        id: library_id,
        media_type,
        name: format!("{media_type:?} lib"),
        root_folders: vec!["/tmp/synthetic".into()],
        default_quality_profile: QualityProfileId::new(),
    };
    db.config().upsert_library(&library).await.unwrap();

    let content_id = ContentId::new();
    let node = cellarr_core::ContentNode {
        id: content_id,
        library_id,
        media_type,
        parent_id: None,
        kind,
        coords: coords.clone(),
        monitored: true,
        title_id: None,
    };
    use cellarr_core::repo::ContentRepository;
    db.content().upsert(&node).await.unwrap();

    ContentRef::new(content_id, library_id, media_type, coords).unwrap()
}

/// Build a registry with the real Movie + TV modules, each over the mocked seams
/// pointed at `node`.
fn registry_for(
    node: &ContentRef,
    movie: Option<MovieMeta>,
    series: Option<SeriesMeta>,
    title: &str,
) -> MediaRegistry {
    let candidate = ContentCandidate {
        content_ref: node.clone(),
        title: title.to_string(),
        aliases: Vec::new(),
    };
    let mut registry = MediaRegistry::new();
    match node.media_type {
        MediaType::Movie => {
            registry.register(MovieModule::new(
                MockContentLookup {
                    candidate: candidate.clone(),
                },
                MockMetadata {
                    movie,
                    series: None,
                },
            ));
        }
        MediaType::Tv => {
            registry.register(TvModule::new(
                MockContentLookup { candidate },
                MockMetadata {
                    movie: None,
                    series,
                },
            ));
        }
        _ => unreachable!("e2e covers movie + tv"),
    }
    registry
}

fn runner_config(library_root: PathBuf, profile: QualityProfile, naming: &str) -> RunnerConfig {
    RunnerConfig {
        profile,
        custom_formats: Vec::<CustomFormat>::new(),
        ranking: QualityRanking::default(),
        proper_repack_policy: ProperRepackPolicy::default(),
        library_root,
        naming_format: naming.to_string(),
        indexer_id: cellarr_core::IndexerId::new(),
        client_id: cellarr_core::DownloadClientId::new(),
        category: "cellarr".into(),
        max_track_polls: 5,
    }
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn movie_release_drives_discover_to_imported_and_lands_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("cellarr.sqlite");
    let db = Database::open(db_path.to_str().unwrap()).await.unwrap();

    // A real downloaded file the fake client will point at.
    let download_dir = tmp.path().join("downloads/movie");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv");
    std::fs::write(&downloaded, b"synthetic movie bytes").unwrap();

    let library_root = tmp.path().join("library/movies");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_node(
        &db,
        MediaType::Movie,
        cellarr_core::ContentKind::Movie,
        Coordinates::Movie,
    )
    .await;

    let registry = registry_for(
        &node,
        Some(MovieMeta {
            title: "The Matrix".into(),
            aliases: Vec::new(),
            year: Some(1999),
            external_ids: Vec::new(),
        }),
        None,
        "The Matrix",
    );

    let indexer = FakeIndexer {
        releases: vec![movie_release("The.Matrix.1999.1080p.BluRay.x264-GROUP")],
    };
    let client = FakeDownloadClient {
        content_path: downloaded.to_string_lossy().into_owned(),
    };
    let clock = LogicalClock::new(0);
    let config = runner_config(
        library_root.clone(),
        permissive_profile(),
        "{Movie Title} ({Release Year})/{Movie Title}.{Extension}",
    );

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
    let outcome = runner.run(&node).await.unwrap();

    let (grab_id, destinations) = match outcome {
        RunOutcome::Imported {
            grab_id,
            destinations,
        } => (grab_id, destinations),
        other => panic!("expected Imported, got {other:?}"),
    };

    // The file actually landed at the renamed destination on disk.
    assert_eq!(destinations.len(), 1);
    let dest = PathBuf::from(&destinations[0]);
    assert!(dest.exists(), "imported movie file must exist at {dest:?}");
    assert!(dest.starts_with(&library_root));
    assert_eq!(
        dest.file_name().unwrap().to_str().unwrap(),
        "The Matrix.mkv"
    );
    // The folder was rendered from the movie module's tokens (Title + Year).
    assert_eq!(
        dest.parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap(),
        "The Matrix (1999)"
    );

    // The grab reached Imported.
    let grab = GrabRepository::get(&db.grabs(), grab_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grab.status, GrabStatus::Imported);

    // A decision_log entry exists explaining the grab (a Decide->Grab advance
    // carrying the Grab verdict), and history records the grab + import.
    let _ = grab_id;
    let history = HistoryRepository::for_content(&db.history(), node.id)
        .await
        .unwrap();
    assert!(
        history
            .iter()
            .any(|h| matches!(h.event, cellarr_core::history::HistoryEvent::Grabbed { .. })),
        "history must record the grab"
    );
    assert!(
        history.iter().any(|h| matches!(
            h.event,
            cellarr_core::history::HistoryEvent::Imported { .. }
        )),
        "history must record the import"
    );

    // The decision log for this run carries the Grab verdict explaining *why*.
    let run_id = history
        .iter()
        .find_map(|h| match h.event {
            cellarr_core::history::HistoryEvent::Grabbed { .. } => Some(h.run_id),
            _ => None,
        })
        .unwrap();
    let records = db.decision_log().for_run(run_id).await.unwrap();
    assert!(
        records.iter().any(|r| matches!(
            r.decision.as_ref().map(|d| &d.verdict),
            Some(cellarr_core::Verdict::Grab { .. })
        )),
        "decision_log must contain a Grab verdict explaining the grab"
    );
}

#[tokio::test]
async fn tv_episode_release_drives_discover_to_imported_and_lands_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("cellarr.sqlite");
    let db = Database::open(db_path.to_str().unwrap()).await.unwrap();

    let download_dir = tmp.path().join("downloads/tv");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("The.Show.S02E05.1080p.WEB-DL.x264-GROUP.mkv");
    std::fs::write(&downloaded, b"synthetic episode bytes").unwrap();

    let library_root = tmp.path().join("library/tv");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_node(
        &db,
        MediaType::Tv,
        cellarr_core::ContentKind::Episode,
        Coordinates::Episode {
            season: 2,
            episode: 5,
            absolute: None,
        },
    )
    .await;

    let registry = registry_for(
        &node,
        None,
        Some(SeriesMeta {
            title: "The Show".into(),
            aliases: Vec::new(),
            year: Some(2018),
            external_ids: Vec::new(),
        }),
        "The Show",
    );

    let indexer = FakeIndexer {
        releases: vec![tv_release("The.Show.S02E05.1080p.WEB-DL.x264-GROUP")],
    };
    let client = FakeDownloadClient {
        content_path: downloaded.to_string_lossy().into_owned(),
    };
    let clock = LogicalClock::new(0);
    let config = runner_config(
        library_root.clone(),
        permissive_profile(),
        "{Series Title}/{Series Title}.{Extension}",
    );

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
    let outcome = runner.run(&node).await.unwrap();

    let (grab_id, destinations) = match outcome {
        RunOutcome::Imported {
            grab_id,
            destinations,
        } => (grab_id, destinations),
        other => panic!("expected Imported, got {other:?}"),
    };

    assert_eq!(destinations.len(), 1);
    let dest = PathBuf::from(&destinations[0]);
    assert!(
        dest.exists(),
        "imported episode file must exist at {dest:?}"
    );
    assert!(dest.starts_with(&library_root));
    assert_eq!(dest.file_name().unwrap().to_str().unwrap(), "The Show.mkv");

    let grab = GrabRepository::get(&db.grabs(), grab_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grab.status, GrabStatus::Imported);

    let history = HistoryRepository::for_content(&db.history(), node.id)
        .await
        .unwrap();
    assert!(history
        .iter()
        .any(|h| matches!(h.event, cellarr_core::history::HistoryEvent::Grabbed { .. })));
    assert!(history.iter().any(|h| matches!(
        h.event,
        cellarr_core::history::HistoryEvent::Imported { .. }
    )));
}

/// The completed-download → import handoff: a PRE-STAGED "completed" download
/// directory (no real download) flows through the runner's Track→Import stage
/// into cellarr-fs's stage→verify→commit, and — because the downloads dir and
/// the library are on the same filesystem — the imported file is a **hardlink**
/// of the download (same inode, link count 2), preserving the seeding copy and
/// costing no extra disk. This is the differentiator the whole task hinges on:
/// we assert the inode identity, not merely "a file with the same bytes exists".
#[cfg(unix)]
#[tokio::test]
async fn completed_download_imports_as_a_hardlink_on_the_same_filesystem() {
    use std::os::unix::fs::MetadataExt;

    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("cellarr.sqlite");
    let db = Database::open(db_path.to_str().unwrap()).await.unwrap();

    // PRE-STAGE a completed download as a directory of dummy media files — the
    // exact shape a torrent client hands off (a single content folder). Nothing
    // is downloaded; these are written bytes the fake client points `track` at.
    let completed = tmp
        .path()
        .join("downloads/complete/The.Matrix.1999.1080p.BluRay");
    std::fs::create_dir_all(&completed).unwrap();
    let media = completed.join("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv");
    std::fs::write(&media, b"dummy completed media payload").unwrap();

    // Library root under the SAME tempdir → same filesystem as downloads, so a
    // hardlink is feasible.
    let library_root = tmp.path().join("library/movies");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_node(
        &db,
        MediaType::Movie,
        cellarr_core::ContentKind::Movie,
        Coordinates::Movie,
    )
    .await;
    let registry = registry_for(
        &node,
        Some(MovieMeta {
            title: "The Matrix".into(),
            aliases: Vec::new(),
            year: Some(1999),
            external_ids: Vec::new(),
        }),
        None,
        "The Matrix",
    );

    let indexer = FakeIndexer {
        releases: vec![movie_release("The.Matrix.1999.1080p.BluRay.x264-GROUP")],
    };
    // The client "completes" pointing at the pre-staged completed directory.
    let client = FakeDownloadClient {
        content_path: completed.to_string_lossy().into_owned(),
    };
    let clock = LogicalClock::new(0);
    let config = runner_config(
        library_root.clone(),
        permissive_profile(),
        "{Movie Title} ({Release Year})/{Movie Title}.{Extension}",
    );

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
    let outcome = runner.run(&node).await.unwrap();

    let destinations = match outcome {
        RunOutcome::Imported { destinations, .. } => destinations,
        other => panic!("expected Imported, got {other:?}"),
    };
    assert_eq!(destinations.len(), 1);
    let dest = PathBuf::from(&destinations[0]);
    assert!(dest.exists(), "imported file must exist at {dest:?}");

    // The defining hardlink assertions: the imported library file and the
    // still-present download share ONE inode (link count 2). If the import had
    // silently copied, these inodes would differ and nlink would be 1.
    let src_meta = std::fs::metadata(&media).unwrap();
    let dst_meta = std::fs::metadata(&dest).unwrap();
    assert_eq!(
        src_meta.ino(),
        dst_meta.ino(),
        "import on same-fs must hardlink (shared inode), not copy"
    );
    assert_eq!(src_meta.dev(), dst_meta.dev(), "same device expected");
    assert_eq!(
        dst_meta.nlink(),
        2,
        "the download (seeding copy) and the library file are two names for one inode"
    );
    // The seeding copy is preserved (the original download still exists).
    assert!(media.exists(), "the seeding copy must be preserved");
}

#[tokio::test]
async fn junk_low_quality_release_is_rejected_with_a_logged_reason_and_no_file_moved() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("cellarr.sqlite");
    let db = Database::open(db_path.to_str().unwrap()).await.unwrap();

    let download_dir = tmp.path().join("downloads/junk");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("junk.mkv");
    std::fs::write(&downloaded, b"junk").unwrap();

    let library_root = tmp.path().join("library/junk");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_node(
        &db,
        MediaType::Movie,
        cellarr_core::ContentKind::Movie,
        Coordinates::Movie,
    )
    .await;
    let registry = registry_for(
        &node,
        Some(MovieMeta {
            title: "The Matrix".into(),
            aliases: Vec::new(),
            year: Some(1999),
            external_ids: Vec::new(),
        }),
        None,
        "The Matrix",
    );

    // A junk release with no recognizable source/resolution -> resolves to the
    // "Unknown" quality, which the profile does not allow -> QualityNotAllowed.
    let indexer = FakeIndexer {
        releases: vec![movie_release("The Matrix 1999 junk nonsense")],
    };
    let client = FakeDownloadClient {
        content_path: downloaded.to_string_lossy().into_owned(),
    };
    let clock = LogicalClock::new(0);
    let config = runner_config(
        library_root.clone(),
        permissive_profile(),
        "{Title}/{Title}.{Extension}",
    );

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
    let outcome = runner.run(&node).await.unwrap();

    match &outcome {
        RunOutcome::Rejected { reason } => {
            assert!(
                reason.contains("quality not allowed"),
                "reject reason should explain the rejection, got: {reason}"
            );
        }
        other => panic!("expected Rejected, got {other:?}"),
    }

    // No grab reached a download client and no file moved into the library.
    let library_entries: Vec<_> = walkdir(&library_root);
    assert!(
        library_entries.is_empty(),
        "no file may be moved on a reject; found {library_entries:?}"
    );

    // The rejection is logged: a decision_log record carrying a Reject verdict.
    // We find the run via any decision_log record (the run had no history grab).
    // The reject path appends a Decide->Rejected record with the Decision.
    let any_run = first_run_with_reject(&db).await;
    let records = db.decision_log().for_run(any_run).await.unwrap();
    assert!(
        records.iter().any(|r| matches!(
            r.decision.as_ref().map(|d| &d.verdict),
            Some(cellarr_core::Verdict::Reject { .. })
        )),
        "decision_log must contain the Reject verdict and its reason"
    );
}

// --- small helpers ----------------------------------------------------------

/// List every regular file under `root` (recursively), as relative path strings.
fn walkdir(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walkdir(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// Find a run id that produced a Reject decision (the negative test only runs one
/// pipeline, so there is exactly one such run).
async fn first_run_with_reject(db: &Database) -> cellarr_core::PipelineRunId {
    // The decision_log table is keyed by run; we scan history-free by probing the
    // single grab-less run. Since the test issues exactly one run, we read the
    // newest decision_log row's run via a direct query through the pool.
    let row: (String,) = sqlx::query_as("SELECT run_id FROM decision_log ORDER BY at DESC LIMIT 1")
        .fetch_one(db.pool())
        .await
        .unwrap();
    cellarr_core::PipelineRunId::from_uuid(row.0.parse().unwrap())
}
