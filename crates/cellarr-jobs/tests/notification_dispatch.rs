//! Notification-provider dispatch, end-to-end through the real pipeline runner.
//!
//! HERMETIC: no live services. A recording mock [`NotificationSender`] stands in
//! for every provider; the pipeline uses the same FAKE indexer / FAKE download
//! client / temp-dir cellarr-fs / real cellarr-parse+decide+media+db harness the
//! webhook e2e test uses (movie path only).
//!
//! Asserts:
//!  - [`ProviderNotifier::dispatch`] routes a message only to the sender whose
//!    `kind` matches the notification, and respects per-event toggles;
//!  - a pipeline Grab + Import fires the `Grab` then `Import` provider events to a
//!    subscribed provider, carrying the subject + release + imported file;
//!  - a notification subscribed only to `download` does NOT receive the `Grab`;
//!  - a **failing** provider does not break the pipeline (the import still
//!    succeeds and the run is `Imported`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use cellarr_core::{
    repo::ContentRepository, ContentId, ContentRef, Coordinates, CustomFormat, LibraryId,
    MediaType, NotificationConfig, NotificationEvent, NotificationMessage, NotificationSender,
    Protocol, QualityProfile, QualityProfileId, QualityRanking, Release, SearchTerms,
};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_jobs::clock::LogicalClock;
use cellarr_jobs::runner::{PipelineRunner, RunOutcome, RunnerConfig};
use cellarr_jobs::ProviderNotifier;
use cellarr_media::{
    ContentCandidate, ContentLookup, MediaRegistry, MetadataLookup, MovieMeta, MovieModule,
    SeriesMeta,
};
use serde_json::json;

// ---------------------------------------------------------------------------
// A recording mock NotificationSender.
// ---------------------------------------------------------------------------

/// One recorded delivery: the notification name + kind and the message event.
#[derive(Debug, Clone)]
struct Delivered {
    name: String,
    event: NotificationEvent,
    message: NotificationMessage,
}

/// A sender of a fixed `kind` that records every delivery. Optionally fails every
/// send (to prove a failing provider never breaks the pipeline).
struct RecordingSender {
    kind: &'static str,
    fail: bool,
    delivered: Arc<Mutex<Vec<Delivered>>>,
}

impl RecordingSender {
    fn new(kind: &'static str) -> (Self, Arc<Mutex<Vec<Delivered>>>) {
        let delivered = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                kind,
                fail: false,
                delivered: Arc::clone(&delivered),
            },
            delivered,
        )
    }

    fn failing(kind: &'static str) -> (Self, Arc<Mutex<Vec<Delivered>>>) {
        let delivered = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                kind,
                fail: true,
                delivered: Arc::clone(&delivered),
            },
            delivered,
        )
    }
}

#[async_trait]
impl NotificationSender for RecordingSender {
    fn kind(&self) -> &'static str {
        self.kind
    }

    async fn send(
        &self,
        config: &NotificationConfig,
        message: &NotificationMessage,
    ) -> Result<(), String> {
        self.delivered.lock().unwrap().push(Delivered {
            name: config.name.clone(),
            event: message.event,
            message: message.clone(),
        });
        if self.fail {
            Err("simulated provider failure".to_string())
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Direct dispatch tests (routing + toggles), no pipeline.
// ---------------------------------------------------------------------------

async fn register_notification(db: &Database, name: &str, kind: &str, on_events: Vec<String>) {
    let n = NotificationConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_string(),
        kind: kind.to_string(),
        enabled: true,
        on_events,
        settings: json!({}),
    };
    db.config().upsert_notification(&n).await.unwrap();
}

#[tokio::test]
async fn dispatch_routes_by_kind_and_respects_toggles() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();

    // A Discord notification subscribed to grab only, and a Telegram one to all.
    register_notification(&db, "disc", "discord", vec!["grab".into()]).await;
    register_notification(&db, "tele", "telegram", vec![]).await;
    // A disabled Discord notification that must never fire.
    let disabled = NotificationConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name: "off".into(),
        kind: "discord".into(),
        enabled: false,
        on_events: vec![],
        settings: json!({}),
    };
    db.config().upsert_notification(&disabled).await.unwrap();

    let (discord, disc_log) = RecordingSender::new("discord");
    let (telegram, tele_log) = RecordingSender::new("telegram");
    let notifier = ProviderNotifier::new(
        db.clone(),
        vec![Arc::new(discord), Arc::new(telegram)],
        "cellarr",
    );

    // A Grab: discord (grab-subscribed) + telegram (all) fire; disabled does not.
    notifier
        .dispatch(NotificationMessage::new(NotificationEvent::Grab, ""))
        .await;
    // An Import: discord is NOT subscribed (grab only); telegram fires.
    notifier
        .dispatch(NotificationMessage::new(NotificationEvent::Import, ""))
        .await;

    let disc = disc_log.lock().unwrap();
    let tele = tele_log.lock().unwrap();
    assert_eq!(disc.len(), 1, "discord should fire once (grab only)");
    assert_eq!(disc[0].name, "disc");
    assert_eq!(disc[0].event, NotificationEvent::Grab);
    assert_eq!(tele.len(), 2, "telegram subscribes to all events");
    // The dispatcher stamps the instance name when the message leaves it empty.
    assert_eq!(tele[0].message.instance_name, "cellarr");
}

#[tokio::test]
async fn dispatch_skips_unrouted_kinds() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();
    // A kind with no registered sender (the webhook kind is delivered elsewhere).
    register_notification(&db, "wh", "webhook", vec![]).await;
    let (discord, disc_log) = RecordingSender::new("discord");
    let notifier = ProviderNotifier::new(db.clone(), vec![Arc::new(discord)], "cellarr");
    notifier
        .dispatch(NotificationMessage::new(NotificationEvent::Grab, ""))
        .await;
    assert!(
        disc_log.lock().unwrap().is_empty(),
        "no discord notification configured; nothing should fire"
    );
}

#[tokio::test]
async fn dispatch_health_fires_health_event() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();
    register_notification(&db, "disc", "discord", vec!["health".into()]).await;
    let (discord, disc_log) = RecordingSender::new("discord");
    let notifier = ProviderNotifier::new(db.clone(), vec![Arc::new(discord)], "cellarr");
    notifier
        .dispatch_health(false, "error", "disk full", "DownloadClientCheck")
        .await;
    let disc = disc_log.lock().unwrap();
    assert_eq!(disc.len(), 1);
    assert_eq!(disc[0].event, NotificationEvent::HealthIssue);
    let health = disc[0].message.health.as_ref().unwrap();
    assert_eq!(health.message, "disk full");
}

// ---------------------------------------------------------------------------
// Full-pipeline tests.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pipeline_fires_grab_then_import_to_subscribed_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();
    register_notification(&db, "disc", "discord", vec![]).await; // all events
                                                                 // A second provider subscribed only to import (download) — must NOT see Grab.
    register_notification(&db, "tele", "telegram", vec!["download".into()]).await;

    let download_dir = tmp.path().join("downloads");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("The.Matrix.1999.1080p.BluRay.x264-GRP.mkv");
    std::fs::write(&downloaded, b"bytes").unwrap();
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_movie_node(&db, "The Matrix").await;
    let registry = movie_registry(&node, "The Matrix");
    let indexer = FakeIndexer {
        releases: vec![movie_release(
            "The.Matrix.1999.1080p.BluRay.x264-GRP",
            "guid-ok",
        )],
    };
    let client = FakeDownloadClient {
        content_path: Some(downloaded.to_string_lossy().into_owned()),
    };
    let clock = LogicalClock::new(0);
    let config = runner_config(library_root.clone());

    let (discord, disc_log) = RecordingSender::new("discord");
    let (telegram, tele_log) = RecordingSender::new("telegram");
    let notifier = ProviderNotifier::new(
        db.clone(),
        vec![Arc::new(discord), Arc::new(telegram)],
        "cellarr",
    );

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config)
        .with_provider_notifier(notifier);
    let outcome = runner.run(&node).await.unwrap();
    assert!(
        matches!(outcome, RunOutcome::Imported { .. }),
        "{outcome:?}"
    );

    let disc = disc_log.lock().unwrap();
    let events: Vec<NotificationEvent> = disc.iter().map(|d| d.event).collect();
    assert!(
        events.contains(&NotificationEvent::Grab),
        "discord should see Grab, got {events:?}"
    );
    assert!(
        events.contains(&NotificationEvent::Import),
        "discord should see Import, got {events:?}"
    );
    // The Grab message carries the subject + release.
    let grab = disc
        .iter()
        .find(|d| d.event == NotificationEvent::Grab)
        .unwrap();
    let subject = grab.message.subject.as_ref().unwrap();
    assert_eq!(subject.title, "The Matrix");
    assert_eq!(subject.media_type, Some(MediaType::Movie));
    assert_eq!(
        grab.message.release.as_ref().unwrap().release_title,
        "The.Matrix.1999.1080p.BluRay.x264-GRP"
    );
    // The Import message carries the imported file path.
    let import = disc
        .iter()
        .find(|d| d.event == NotificationEvent::Import)
        .unwrap();
    assert!(import
        .message
        .files
        .iter()
        .any(|f| f.ends_with("The Matrix.mkv")));

    // The download-only provider saw the Import but NOT the Grab.
    let tele = tele_log.lock().unwrap();
    let tele_events: Vec<NotificationEvent> = tele.iter().map(|d| d.event).collect();
    assert!(
        !tele_events.contains(&NotificationEvent::Grab),
        "{tele_events:?}"
    );
    assert!(
        tele_events.contains(&NotificationEvent::Import),
        "{tele_events:?}"
    );
}

#[tokio::test]
async fn failing_provider_does_not_break_import() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();
    register_notification(&db, "broken", "discord", vec![]).await;

    let download_dir = tmp.path().join("downloads");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("The.Matrix.1999.1080p.BluRay.x264-GRP.mkv");
    std::fs::write(&downloaded, b"bytes").unwrap();
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_movie_node(&db, "The Matrix").await;
    let registry = movie_registry(&node, "The Matrix");
    let indexer = FakeIndexer {
        releases: vec![movie_release(
            "The.Matrix.1999.1080p.BluRay.x264-GRP",
            "guid-ok",
        )],
    };
    let client = FakeDownloadClient {
        content_path: Some(downloaded.to_string_lossy().into_owned()),
    };
    let clock = LogicalClock::new(0);
    let config = runner_config(library_root.clone());

    let (broken, log) = RecordingSender::failing("discord");
    let notifier = ProviderNotifier::new(db.clone(), vec![Arc::new(broken)], "cellarr");
    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config)
        .with_provider_notifier(notifier);

    // The provider fails on every send, yet the import completes successfully.
    let outcome = runner.run(&node).await.unwrap();
    assert!(
        matches!(outcome, RunOutcome::Imported { .. }),
        "a failing notifier must not break the import: {outcome:?}"
    );
    // It was still attempted (the failure was swallowed, not skipped).
    assert!(!log.lock().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Pipeline harness (movie path only) — mirrors tests/webhook_and_blocklist.rs.
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

struct FakeDownloadClient {
    content_path: Option<String>,
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
        match &self.content_path {
            Some(path) => Ok(cellarr_core::DownloadStatus {
                state: cellarr_core::DownloadState::Completed,
                progress: 1.0,
                content_path: Some(path.clone()),
                ratio: Some(1.0),
                seeding_time_secs: Some(0),
                peers: Some(10),
                error_string: None,
            }),
            None => Ok(cellarr_core::DownloadStatus::from_state(
                cellarr_core::DownloadState::Failed,
            )),
        }
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

async fn seed_movie_node(db: &Database, title: &str) -> ContentRef {
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
    db.content().index_title(content_id, title).await.unwrap();

    ContentRef::new(content_id, library_id, MediaType::Movie, Coordinates::Movie).unwrap()
}

fn movie_registry(node: &ContentRef, title: &str) -> MediaRegistry {
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
        permissions: Default::default(),
        extra_files: Default::default(),
        indexer_criteria: Default::default(),
    }
}
