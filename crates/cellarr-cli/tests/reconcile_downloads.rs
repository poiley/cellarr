//! The reconcile sweep's behavior against reality: prove
//! `JobKind::ReconcileDownloads` finalizes and cleans in-flight grabs correctly.
//!
//! The reconcile is the recovery path for grabs a normal run left mid-flight (a
//! transient client fault, a process restart): it walks every non-terminal grab
//! and, per grab, either finalizes a completed-but-unimported download, imports
//! away a redundant one, or blocklists a dead one (hard-failed, gone from the
//! client, or long-stalled with no peers) — while leaving a healthy download and
//! a freshly-added peer-less one untouched.
//!
//! Each case seeds the DB, assembles the REAL [`LivePipelineHandler`] over a FAKE
//! [`PipelineEnv`] whose fake client's status is keyed by the grab's download id,
//! drives one `handle(ReconcileDownloads)`, and asserts the grab's terminal state
//! (and, for the finalize case, a real file on disk). No Docker, no network.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use cellarr_cli::pipeline::{LivePipelineHandler, PipelineEnv};
use cellarr_core::blocklist::BlocklistRepository;
use cellarr_core::repo::{GrabRepository, MediaFileRepository};
use cellarr_core::{
    ContentId, ContentKind, ContentNode, ContentRef, Coordinates, CustomFormat, DownloadClientId,
    DownloadState, DownloadStatus, GrabRequest, GrabStatus, Indexer, IndexerId, Library, LibraryId,
    MediaFile, MediaFileId, MediaType, Protocol, Quality, QualityProfile, QualityProfileId,
    QualityRanking, Release, ReleaseType, SearchTerms,
};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_jobs::runner::RunnerConfig;
use cellarr_jobs::{JobHandler, JobKind, JobResult};
use cellarr_media::{
    ContentCandidate, ContentLookup, MediaRegistry, MetadataLookup, MovieMeta, MovieModule,
    SeriesMeta,
};

// ---------------------------------------------------------------------------
// Fakes: a client whose status is keyed by the grab's download id, and an env
// that hands the handler that client + a temp-rooted runner config.
// ---------------------------------------------------------------------------

/// A FAKE download client whose `status` branches on the download id, so one
/// client drives every reconcile scenario in a single sweep:
/// - `completed-dl` → Completed, pointing at a real temp file (finalize import).
/// - `failed-dl`    → Failed (dead → blocklist).
/// - `gone-dl`      → `NotFound` error (gone → blocklist).
/// - `healthy-dl`   → Downloading with peers (leave).
/// - `peerless-dl`  → Downloading with zero peers (leave if young, blocklist if aged).
struct ReconcileClient {
    completed_path: String,
}

#[async_trait]
impl cellarr_core::DownloadClient for ReconcileClient {
    // The typed download error, so `NotFound` downcasts through the reconcile's
    // `is_download_gone` check exactly as the live client's does.
    type Error = cellarr_download::DownloadError;
    fn name(&self) -> &str {
        "reconcile-fake"
    }
    async fn add(&self, _grab: &GrabRequest) -> Result<String, Self::Error> {
        Ok("dl".into())
    }
    async fn status(&self, id: &str) -> Result<DownloadStatus, Self::Error> {
        let base = |state, progress, peers| DownloadStatus {
            state,
            progress,
            content_path: None,
            ratio: Some(1.0),
            seeding_time_secs: Some(0),
            peers,
            error_string: None,
        };
        match id {
            "completed-dl" => Ok(DownloadStatus {
                content_path: Some(self.completed_path.clone()),
                ..base(DownloadState::Completed, 1.0, Some(0))
            }),
            "failed-dl" => Ok(base(DownloadState::Failed, 0.0, Some(1))),
            "gone-dl" => Err(cellarr_download::DownloadError::NotFound(id.into())),
            "healthy-dl" => Ok(base(DownloadState::Downloading, 0.5, Some(10))),
            "peerless-dl" => Ok(base(DownloadState::Downloading, 0.0, Some(0))),
            _ => Ok(base(DownloadState::Downloading, 0.0, None)),
        }
    }
    async fn remove(&self, _id: &str, _delete: bool) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// A FAKE indexer that offers the given release on every search (or nothing when
/// `offer` is `None`). The concurrency-cap tests hand it a grabbable release so a
/// run would grab absent the cap.
struct FakeIndexer {
    offer: Option<Release>,
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
        Ok(self.offer.clone().into_iter().collect())
    }
    async fn latest(&self) -> Result<Vec<Release>, Self::Error> {
        Ok(self.offer.clone().into_iter().collect())
    }
}

/// A FAKE [`PipelineEnv`] handing the handler the fake client + a temp-rooted
/// runner config (the import target for the finalize case).
struct FakeEnv {
    completed_path: String,
    library_root: PathBuf,
    profile: QualityProfile,
    offer: Option<Release>,
}

#[async_trait]
impl PipelineEnv for FakeEnv {
    type Indexer = FakeIndexer;
    type Client = ReconcileClient;

    async fn resolve(
        &self,
        _content: &ContentRef,
    ) -> Result<Option<(Self::Indexer, Self::Client, RunnerConfig)>, String> {
        let config = RunnerConfig {
            content_tag_ids: Vec::new(),
            profile: self.profile.clone(),
            custom_formats: Vec::<CustomFormat>::new(),
            ranking: QualityRanking::default(),
            proper_repack_policy: ProperRepackPolicy::default(),
            library_root: self.library_root.clone(),
            naming_format: "{Movie Title} ({Release Year})/{Movie Title}.{Extension}".into(),
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
            FakeIndexer {
                offer: self.offer.clone(),
            },
            ReconcileClient {
                completed_path: self.completed_path.clone(),
            },
            config,
        )))
    }
}

// ---------------------------------------------------------------------------
// Media registry: a Movie module that resolves the seeded node (so the finalize
// case can render the destination name from the movie tokens).
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

/// Seed a monitored, file-less movie node.
async fn seed_movie(db: &Database) -> ContentRef {
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
        tags: Vec::new(),
        id: content_id,
        library_id,
        media_type: MediaType::Movie,
        parent_id: None,
        kind: ContentKind::Movie,
        series_type: cellarr_core::SeriesType::Standard,
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

/// Seed a non-terminal grab for `content` with the given download id, returning
/// its id. The download id selects the fake client's status branch.
async fn seed_grab(db: &Database, content: &ContentRef, download_id: &str) -> cellarr_core::GrabId {
    let request = GrabRequest {
        content_ref: content.clone(),
        release: Release {
            indexer_id: IndexerId::new(),
            title: "The.Matrix.1999.1080p.BluRay.x264-GROUP".into(),
            download_url: "magnet:?xt=urn:btih:abc".into(),
            guid: Some("the-matrix-1999".into()),
            protocol: Protocol::Torrent,
            size: Some(8_000_000_000),
            seeders: Some(100),
            indexer_flags: Vec::new(),
        },
        indexer_id: IndexerId::new(),
        client_id: DownloadClientId::new(),
        category: "cellarr".into(),
        release_type: Some(ReleaseType::Movie),
    };
    let id = db.grabs().create(&request).await.unwrap();
    db.grabs().set_download_id(id, download_id).await.unwrap();
    db.grabs().set_status(id, GrabStatus::Sent).await.unwrap();
    id
}

/// Build the handler over the fake env, with the completed-download file at
/// `completed_path`. No release offered, no download cap.
fn handler(db: &Database, node: &ContentRef, completed_path: String, library_root: PathBuf) -> impl JobHandler {
    handler_with(db, node, completed_path, library_root, None, None)
}

/// Build the handler with an indexer that offers `offer` and an in-flight
/// download `cap` — for exercising the sweep's grab + concurrency-cap paths.
fn handler_with(
    db: &Database,
    node: &ContentRef,
    completed_path: String,
    library_root: PathBuf,
    offer: Option<Release>,
    cap: Option<u32>,
) -> impl JobHandler {
    let env = FakeEnv {
        completed_path,
        library_root,
        profile: permissive_profile(),
        offer,
    };
    LivePipelineHandler::new(
        db.clone(),
        Arc::new(movie_registry(node)),
        cellarr_api::events::EventBus::default(),
        env,
    )
    .with_max_active_downloads(cap)
}

async fn status_of(db: &Database, id: cellarr_core::GrabId) -> GrabStatus {
    db.grabs().get(id).await.unwrap().unwrap().status
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// A download the client reports **complete** but that was never imported gets
/// finalized: the file lands under the library root and the grab goes Imported.
#[tokio::test]
async fn reconcile_finalizes_completed_but_unimported_download() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();

    let download_dir = tmp.path().join("downloads");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv");
    std::fs::write(&downloaded, b"synthetic movie bytes").unwrap();
    let library_root = tmp.path().join("library/movies");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_movie(&db).await;
    let grab = seed_grab(&db, &node, "completed-dl").await;

    let h = handler(
        &db,
        &node,
        downloaded.to_string_lossy().into_owned(),
        library_root.clone(),
    );
    assert!(matches!(
        h.handle(&JobKind::ReconcileDownloads).await,
        JobResult::Success
    ));

    assert_eq!(
        status_of(&db, grab).await,
        GrabStatus::Imported,
        "a completed download is finalized to Imported"
    );
    // The file was imported on disk, and a media_file row now satisfies the node.
    let imported = find_one_file(&library_root).expect("an imported file under the library root");
    assert_eq!(std::fs::read(&imported).unwrap(), b"synthetic movie bytes");
    let files = db.media_files().list_for_content(node.id).await.unwrap();
    assert_eq!(files.len(), 1, "the finalized import recorded a media_file");
}

/// A grab for content that is already satisfied by a file is redundant: the
/// reconcile marks it Imported (and drops the duplicate download) without
/// blocklisting anything.
#[tokio::test]
async fn reconcile_imports_redundant_grab_when_content_already_satisfied() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_movie(&db).await;
    // The node already has a file on record.
    let file = MediaFile {
        id: MediaFileId::new(),
        path: library_root
            .join("The Matrix (1999).mkv")
            .to_string_lossy()
            .into_owned(),
        size: 42,
        quality: Quality::new("Bluray-1080p", 14),
        languages: Vec::new(),
        media_info: None,
        custom_format_score: None,
        release_type: Some(ReleaseType::Movie),
    };
    db.media_files().create(&file).await.unwrap();
    db.media_files().link(node.id, file.id).await.unwrap();
    let grab = seed_grab(&db, &node, "healthy-dl").await;

    let h = handler(&db, &node, String::new(), library_root);
    let _ = h.handle(&JobKind::ReconcileDownloads).await;

    assert_eq!(
        status_of(&db, grab).await,
        GrabStatus::Imported,
        "a redundant grab for satisfied content is finalized Imported"
    );
    assert!(
        db.blocklist().list().await.unwrap().is_empty(),
        "a redundant grab is not blocklisted"
    );
}

/// A duplicate download whose correctly-named file is ALREADY on disk — but not
/// linked to its node (an earlier import whose media_file row was lost) — must not
/// loop forever failing the import with "destination already exists". The
/// reconcile adopts the existing file onto the node (no byte moved) to satisfy it,
/// marks the grab Imported, and drops the duplicate — never blocklisting.
#[tokio::test]
async fn reconcile_adopts_existing_file_when_duplicate_download_collides() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();
    let download_dir = tmp.path().join("downloads");
    std::fs::create_dir_all(&download_dir).unwrap();

    let node = seed_movie(&db).await;

    // Let a completed download import normally so the destination file lands at the
    // exact scheme path — then drop only its media_file row (the file stays on
    // disk) to reproduce the lost-link state a duplicate would collide with.
    let dl1 = download_dir.join("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv");
    std::fs::write(&dl1, b"library copy").unwrap();
    let grab1 = seed_grab(&db, &node, "completed-dl").await;
    let h1 = handler(
        &db,
        &node,
        dl1.to_string_lossy().into_owned(),
        library_root.clone(),
    );
    let _ = h1.handle(&JobKind::ReconcileDownloads).await;
    assert_eq!(status_of(&db, grab1).await, GrabStatus::Imported);
    let dest = find_one_file(&library_root).expect("imported file under the library root");
    let dest_str = dest.to_string_lossy().into_owned();
    db.media_files().delete_by_path(&dest_str).await.unwrap();
    assert!(
        db.media_files()
            .list_for_content(node.id)
            .await
            .unwrap()
            .is_empty(),
        "the node is now unlinked while its file remains on disk"
    );

    // A NEW duplicate download of the same movie completes; its import collides
    // with the on-disk file. The reconcile must adopt it, not loop.
    let dl2 = download_dir.join("The.Matrix.1999.1080p.BluRay.x264-OTHER.mkv");
    std::fs::write(&dl2, b"duplicate download copy").unwrap();
    let grab2 = seed_grab(&db, &node, "completed-dl").await;
    let h2 = handler(
        &db,
        &node,
        dl2.to_string_lossy().into_owned(),
        library_root.clone(),
    );
    let _ = h2.handle(&JobKind::ReconcileDownloads).await;

    assert_eq!(
        status_of(&db, grab2).await,
        GrabStatus::Imported,
        "the duplicate grab is finalized by adopting the existing file"
    );
    let files = db.media_files().list_for_content(node.id).await.unwrap();
    assert_eq!(files.len(), 1, "the existing file is re-linked to the node");
    assert_eq!(files[0].path, dest_str, "adopted the on-disk file, in place");
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        b"library copy",
        "adopt-in-place kept the original file; the duplicate bytes never overwrote it"
    );
    assert!(
        db.blocklist().list().await.unwrap().is_empty(),
        "adopting a duplicate never blocklists the release"
    );
}

/// A hard-failed download is dead: blocklist it (so the release is not
/// re-grabbed) and mark the grab Blocklisted.
#[tokio::test]
async fn reconcile_blocklists_hard_failed_download() {
    let (tmp, db, node) = fresh_db_with_movie().await;
    let _ = &tmp;
    let grab = seed_grab(&db, &node, "failed-dl").await;

    let h = handler(&db, &node, String::new(), tmp.path().to_path_buf());
    let _ = h.handle(&JobKind::ReconcileDownloads).await;

    assert_eq!(status_of(&db, grab).await, GrabStatus::Blocklisted);
    assert_eq!(
        db.blocklist().list().await.unwrap().len(),
        1,
        "the failed release is blocklisted"
    );
}

/// A download the client no longer knows (a `NotFound` error) is gone: blocklist it.
#[tokio::test]
async fn reconcile_blocklists_download_gone_from_client() {
    let (tmp, db, node) = fresh_db_with_movie().await;
    let grab = seed_grab(&db, &node, "gone-dl").await;

    let h = handler(&db, &node, String::new(), tmp.path().to_path_buf());
    let _ = h.handle(&JobKind::ReconcileDownloads).await;

    assert_eq!(status_of(&db, grab).await, GrabStatus::Blocklisted);
    assert_eq!(db.blocklist().list().await.unwrap().len(), 1);
}

/// A healthy in-flight download (making progress, with peers) is left alone.
#[tokio::test]
async fn reconcile_leaves_healthy_download_untouched() {
    let (tmp, db, node) = fresh_db_with_movie().await;
    let grab = seed_grab(&db, &node, "healthy-dl").await;

    let h = handler(&db, &node, String::new(), tmp.path().to_path_buf());
    let _ = h.handle(&JobKind::ReconcileDownloads).await;

    assert_eq!(
        status_of(&db, grab).await,
        GrabStatus::Sent,
        "a healthy download is not disturbed"
    );
    assert!(db.blocklist().list().await.unwrap().is_empty());
}

/// A freshly-added download reporting zero peers is NOT killed — the age guard
/// protects a download whose client is still warming up / announcing.
#[tokio::test]
async fn reconcile_leaves_new_peerless_download_untouched() {
    let (tmp, db, node) = fresh_db_with_movie().await;
    let grab = seed_grab(&db, &node, "peerless-dl").await;

    let h = handler(&db, &node, String::new(), tmp.path().to_path_buf());
    let _ = h.handle(&JobKind::ReconcileDownloads).await;

    assert_eq!(
        status_of(&db, grab).await,
        GrabStatus::Sent,
        "a young peer-less download is left for the next cycle, not blocklisted"
    );
    assert!(db.blocklist().list().await.unwrap().is_empty());
}

/// A long-stalled download (zero peers, no progress, aged past the stall window)
/// is dead: blocklist it. Backdates the grab's `created_at` past the threshold.
#[tokio::test]
async fn reconcile_blocklists_old_peerless_stalled_download() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("cellarr.sqlite");
    let db = Database::open(db_path.to_str().unwrap()).await.unwrap();
    let node = seed_movie(&db).await;
    let grab = seed_grab(&db, &node, "peerless-dl").await;

    // Backdate the grab well past the 24h stall window via a second handle to the
    // same SQLite file (the repo always stamps `created_at = now`).
    let old = (time::OffsetDateTime::now_utc() - time::Duration::days(3))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", db_path.display()))
        .await
        .unwrap();
    sqlx::query("UPDATE grab SET created_at = ?1 WHERE id = ?2")
        .bind(&old)
        .bind(grab.to_string())
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let h = handler(&db, &node, String::new(), tmp.path().to_path_buf());
    let _ = h.handle(&JobKind::ReconcileDownloads).await;

    assert_eq!(
        status_of(&db, grab).await,
        GrabStatus::Blocklisted,
        "a long-stalled peer-less download is cleaned as dead"
    );
    assert_eq!(db.blocklist().list().await.unwrap().len(), 1);
}

/// A grabbable movie release that the movie registry identifies to the seeded node.
fn matrix_release() -> Release {
    Release {
        indexer_id: IndexerId::new(),
        title: "The.Matrix.1999.1080p.BluRay.x264-GROUP".into(),
        download_url: "magnet:?xt=urn:btih:matrix".into(),
        guid: Some("the-matrix-1999".into()),
        protocol: Protocol::Torrent,
        size: Some(8_000_000_000),
        seeders: Some(100),
        indexer_flags: Vec::new(),
    }
}

/// How many non-terminal grabs exist for `content`.
async fn open_grabs_for(db: &Database, content: cellarr_core::ContentId) -> usize {
    db.grabs()
        .list()
        .await
        .unwrap()
        .iter()
        .filter(|g| {
            (g.request.content_ref.id == content)
                && !matches!(
                    g.status,
                    GrabStatus::Imported | GrabStatus::Failed | GrabStatus::Blocklisted
                )
        })
        .count()
}

/// Under the cap, the sweep grabs a missing item as normal.
#[tokio::test]
async fn sweep_grabs_when_under_the_concurrency_cap() {
    let (tmp, db, node) = fresh_db_with_movie().await;
    let h = handler_with(
        &db,
        &node,
        String::new(),
        tmp.path().to_path_buf(),
        Some(matrix_release()),
        Some(1), // cap 1, nothing in flight yet → under cap
    );
    let _ = h.handle(&JobKind::MissingItemSearch).await;
    assert_eq!(
        open_grabs_for(&db, node.id).await,
        1,
        "a missing item is grabbed when in-flight downloads are below the cap"
    );
}

/// At the cap, the sweep grabs NOTHING new — even a grabbable missing item is
/// deferred to a later sweep (after the reconcile drains completions).
#[tokio::test]
async fn sweep_stops_grabbing_at_the_concurrency_cap() {
    let (tmp, db, node) = fresh_db_with_movie().await;
    // One download already in flight (for other content) → active == cap (1).
    let other = seed_movie(&db).await;
    let _existing = seed_grab(&db, &other, "healthy-dl").await;

    let h = handler_with(
        &db,
        &node,
        String::new(),
        tmp.path().to_path_buf(),
        Some(matrix_release()),
        Some(1),
    );
    let _ = h.handle(&JobKind::MissingItemSearch).await;

    assert_eq!(
        open_grabs_for(&db, node.id).await,
        0,
        "at the concurrency cap, a grabbable missing item is NOT grabbed"
    );
    // The pre-existing in-flight grab is untouched (the cap only gates NEW grabs).
    assert_eq!(open_grabs_for(&db, other.id).await, 1);
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

async fn fresh_db_with_movie() -> (tempfile::TempDir, Database, ContentRef) {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let node = seed_movie(&db).await;
    (tmp, db, node)
}

/// Find exactly one regular file under `root` (recursively).
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
