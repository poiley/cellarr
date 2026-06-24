//! Connect-webhook + failed-download blocklist, end-to-end through the real
//! pipeline runner.
//!
//! HERMETIC: no live services.
//!  - The webhook receiver is a LOCAL mock HTTP server (a tokio TCP listener on an
//!    OS-allocated port) that records the requests it receives. The
//!    [`WebhookSender`] under test is a tiny raw-HTTP client written against
//!    `std::net` (cellarr-jobs has no HTTP client dep), so the whole webhook path
//!    — runner -> WebhookNotifier -> sender -> mock server — is exercised for real
//!    without reqwest or a network.
//!  - The pipeline uses the same FAKE indexer / FAKE download client / temp-dir
//!    cellarr-fs / real cellarr-parse+decide+media+db harness as `pipeline_e2e`.
//!
//! Asserts:
//!  - a pipeline Grab + Import fires the `Grab` and `Download` Connect webhooks to
//!    the registered mock, with the correct `eventType` + body;
//!  - a `Test` event is delivered with `eventType == "Test"`;
//!  - a failed download blocklists the release; a blocklisted release is skipped
//!    on a re-search (the next candidate wins); and a manual remove clears it.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use cellarr_core::blocklist::BlocklistRepository;
use cellarr_core::{
    repo::GrabRepository, ContentId, ContentRef, Coordinates, CustomFormat, GrabStatus, LibraryId,
    MediaType, NotificationConfig, Protocol, QualityProfile, QualityProfileId, QualityRanking,
    Release, SearchTerms, WebhookPayload, WebhookSender,
};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_jobs::clock::LogicalClock;
use cellarr_jobs::notify::WebhookNotifier;
use cellarr_jobs::runner::{PipelineRunner, RunOutcome, RunnerConfig};
use cellarr_media::{
    ContentCandidate, ContentLookup, MediaRegistry, MetadataLookup, MovieMeta, MovieModule,
    SeriesMeta,
};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// A LOCAL mock HTTP server that records POST bodies (no external deps).
// ---------------------------------------------------------------------------

/// A recorded inbound request: the request path and the JSON body.
#[derive(Debug, Clone)]
struct Received {
    path: String,
    body: Value,
}

/// Spawn a one-shot-per-connection mock HTTP server on an OS-allocated port.
/// Returns its `http://127.0.0.1:<port>/cellarr` URL and a shared buffer of the
/// requests it received. The server runs until the test drops, accepting any
/// number of connections and replying `200 OK`.
async fn spawn_mock_server() -> (String, Arc<Mutex<Vec<Received>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let received: Arc<Mutex<Vec<Received>>> = Arc::new(Mutex::new(Vec::new()));
    let recv = Arc::clone(&received);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let recv = Arc::clone(&recv);
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // Read until we have headers + the full body (Content-Length).
                loop {
                    let n = match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(parsed) = parse_request(&buf) {
                        recv.lock().unwrap().push(parsed);
                        break;
                    }
                }
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
                let _ = socket.flush().await;
            });
        }
    });
    (format!("http://127.0.0.1:{port}/cellarr"), received)
}

/// Parse a raw HTTP request once the full body has arrived; `None` until then.
fn parse_request(buf: &[u8]) -> Option<Received> {
    let text = String::from_utf8_lossy(buf);
    let header_end = text.find("\r\n\r\n")?;
    let head = &text[..header_end];
    let body_start = header_end + 4;
    let mut lines = head.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let _method = parts.next()?;
    let path = parts.next()?.to_string();
    let content_length = head
        .lines()
        .find_map(|l| {
            let l = l.to_ascii_lowercase();
            l.strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    let body_bytes = &buf[body_start..];
    if body_bytes.len() < content_length {
        return None; // body not fully received yet
    }
    let body: Value = serde_json::from_slice(&body_bytes[..content_length]).unwrap_or(Value::Null);
    Some(Received { path, body })
}

/// A [`WebhookSender`] that POSTs JSON over a raw blocking `TcpStream` (so the
/// webhook delivery path is real HTTP, with no HTTP-client dependency). Runs the
/// blocking call on a blocking thread so it cooperates with the async runner.
struct RawHttpSender;

#[async_trait]
impl WebhookSender for RawHttpSender {
    async fn send(&self, url: &str, payload: &WebhookPayload) -> Result<(), String> {
        let url = url.to_string();
        let body = serde_json::to_vec(payload).map_err(|e| e.to_string())?;
        tokio::task::spawn_blocking(move || post(&url, &body))
            .await
            .map_err(|e| e.to_string())?
    }
}

/// Minimal blocking HTTP POST of `body` to `url` (`http://host:port/path`).
fn post(url: &str, body: &[u8]) -> Result<(), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or("only http:// supported")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let mut stream = TcpStream::connect(authority).map_err(|e| e.to_string())?;
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        len = body.len()
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.write_all(body).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    let mut resp = String::new();
    let _ = stream.read_to_string(&mut resp);
    if resp.starts_with("HTTP/1.1 200") {
        Ok(())
    } else {
        Err(format!("non-200 response: {resp}"))
    }
}

/// Wait (bounded) until at least `n` requests have been recorded.
async fn wait_for(received: &Arc<Mutex<Vec<Received>>>, n: usize) {
    for _ in 0..200 {
        if received.lock().unwrap().len() >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!(
        "timed out waiting for {n} webhook deliveries (got {})",
        received.lock().unwrap().len()
    );
}

// ---------------------------------------------------------------------------
// Pipeline harness (mirrors tests/pipeline_e2e.rs; movie path only).
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

/// A fake client whose completion behavior is selectable: a content path (= ok)
/// or a hard failure (= the blocklist trigger).
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

/// A self-heal fake: the download for the BAD guid never completes (it reports
/// `Failed`), while the download for the GOOD guid completes with a real on-disk
/// path. The behavior is keyed on the grabbed release's guid (carried in the
/// `GrabRequest` handed to `add`), encoded into the returned download id so
/// `status` can branch on it. It also records every `remove(_, delete_data)` call
/// so a test can assert the dead download was removed with its local data.
struct SelfHealClient {
    bad_guid: String,
    good_path: String,
    /// (download_id, delete_data) for every remove call.
    removed: Arc<Mutex<Vec<(String, bool)>>>,
}

#[async_trait]
impl cellarr_core::traits::DownloadClient for SelfHealClient {
    type Error = FakeClientError;
    fn name(&self) -> &str {
        "self-heal-client"
    }
    async fn add(&self, grab: &cellarr_core::GrabRequest) -> Result<String, Self::Error> {
        // Encode the grabbed guid into the download id so `status` can branch.
        let guid = grab.release.guid.clone().unwrap_or_default();
        Ok(format!("dl::{guid}"))
    }
    async fn status(&self, download_id: &str) -> Result<cellarr_core::DownloadStatus, Self::Error> {
        let guid = download_id.strip_prefix("dl::").unwrap_or(download_id);
        if guid == self.bad_guid {
            // The bad release's download is dead: failed, no peers, no progress.
            return Ok(cellarr_core::DownloadStatus {
                state: cellarr_core::DownloadState::Failed,
                progress: 0.0,
                content_path: None,
                ratio: None,
                seeding_time_secs: None,
                peers: Some(0),
                error_string: Some("tracker reported the torrent is dead".into()),
            });
        }
        Ok(cellarr_core::DownloadStatus {
            state: cellarr_core::DownloadState::Completed,
            progress: 1.0,
            content_path: Some(self.good_path.clone()),
            ratio: Some(1.0),
            seeding_time_secs: Some(0),
            peers: Some(10),
            error_string: None,
        })
    }
    async fn remove(&self, download_id: &str, delete_data: bool) -> Result<(), Self::Error> {
        self.removed
            .lock()
            .unwrap()
            .push((download_id.to_string(), delete_data));
        Ok(())
    }
}

/// A fake whose every download STALLS: it stays `Downloading` forever with no
/// progress and zero peers, so the runner's stall detector fails it. Used to
/// prove a stall (not just a hard `Failed`) drives blocklist + grab-next, and to
/// prove the grab-next cap halts when every candidate stalls.
struct StallingClient;

#[async_trait]
impl cellarr_core::traits::DownloadClient for StallingClient {
    type Error = FakeClientError;
    fn name(&self) -> &str {
        "stalling-client"
    }
    async fn add(&self, _grab: &cellarr_core::GrabRequest) -> Result<String, Self::Error> {
        Ok("dl-stall".to_string())
    }
    async fn status(
        &self,
        _download_id: &str,
    ) -> Result<cellarr_core::DownloadStatus, Self::Error> {
        Ok(cellarr_core::DownloadStatus {
            state: cellarr_core::DownloadState::Downloading,
            progress: 0.0,
            content_path: None,
            ratio: Some(0.0),
            seeding_time_secs: Some(0),
            peers: Some(0),
            error_string: None,
        })
    }
    async fn remove(&self, _download_id: &str, _delete_data: bool) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// A fake whose download sits at end-game (≈99.9% progress, zero peers — the
/// final-piece verify state) for several polls, then flips to `Completed`. Without
/// the near-complete stall exemption this looks like a stall ("no progress, no
/// peers") and gets killed moments before import — the live bug this guards.
struct EndgameClient {
    completed_path: String,
    polls: std::sync::atomic::AtomicU32,
}

#[async_trait]
impl cellarr_core::traits::DownloadClient for EndgameClient {
    type Error = FakeClientError;
    fn name(&self) -> &str {
        "endgame-client"
    }
    async fn add(&self, _grab: &cellarr_core::GrabRequest) -> Result<String, Self::Error> {
        Ok("dl-endgame".to_string())
    }
    async fn status(
        &self,
        _download_id: &str,
    ) -> Result<cellarr_core::DownloadStatus, Self::Error> {
        let n = self.polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // End-game: 99.9% done, no peers — looks exactly like a stall for several
        // polls (more than STALL_MAX_STAGNANT_POLLS) before it completes.
        if n < 5 {
            return Ok(cellarr_core::DownloadStatus {
                state: cellarr_core::DownloadState::Downloading,
                progress: 0.999,
                content_path: None,
                ratio: Some(0.0),
                seeding_time_secs: Some(0),
                peers: Some(0),
                error_string: None,
            });
        }
        Ok(cellarr_core::DownloadStatus {
            state: cellarr_core::DownloadState::Completed,
            progress: 1.0,
            content_path: Some(self.completed_path.clone()),
            ratio: Some(1.0),
            seeding_time_secs: Some(1),
            peers: Some(0),
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
    use cellarr_core::repo::ContentRepository;
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

/// Register a webhook notification pointing at `url`, subscribed to all events.
async fn register_webhook(db: &Database, url: &str) {
    let n = NotificationConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name: "mock-webhook".into(),
        kind: "webhook".into(),
        enabled: true,
        on_events: Vec::new(), // empty = all events
        settings: json!({ "url": url }),
    };
    db.config().upsert_notification(&n).await.unwrap();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pipeline_grab_and_import_fire_connect_webhooks_to_the_mock() {
    let (url, received) = spawn_mock_server().await;

    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();
    register_webhook(&db, &url).await;

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

    let notifier = WebhookNotifier::new(db.clone(), Arc::new(RawHttpSender), "Radarr");
    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config)
        .with_notifier(notifier);
    let outcome = runner.run(&node).await.unwrap();
    assert!(matches!(outcome, RunOutcome::Imported { .. }));

    // Grab + Download (import) + Rename = three deliveries.
    wait_for(&received, 3).await;
    let calls = received.lock().unwrap().clone();
    let event_types: Vec<&str> = calls
        .iter()
        .map(|c| c.body["eventType"].as_str().unwrap_or(""))
        .collect();
    assert!(event_types.contains(&"Grab"), "got {event_types:?}");
    assert!(event_types.contains(&"Download"), "got {event_types:?}");

    // The Grab payload carries the movie subject + release object the ecosystem
    // expects, and was POSTed to the registered path.
    let grab = calls
        .iter()
        .find(|c| c.body["eventType"] == "Grab")
        .unwrap();
    assert_eq!(grab.path, "/cellarr");
    assert_eq!(grab.body["movie"]["title"], "The Matrix");
    assert_eq!(
        grab.body["release"]["releaseTitle"],
        "The.Matrix.1999.1080p.BluRay.x264-GRP"
    );
    assert!(grab.body.get("series").is_none());

    // The Download (import) payload carries the imported file path.
    let imp = calls
        .iter()
        .find(|c| c.body["eventType"] == "Download")
        .unwrap();
    assert!(imp.body["episodeFiles"][0]["path"]
        .as_str()
        .unwrap()
        .ends_with("The Matrix.mkv"));
}

#[tokio::test]
async fn test_event_is_delivered_with_event_type_test() {
    let (url, received) = spawn_mock_server().await;
    let sender = RawHttpSender;
    let payload = WebhookPayload::test("Radarr");
    sender.send(&url, &payload).await.unwrap();
    wait_for(&received, 1).await;
    let calls = received.lock().unwrap().clone();
    assert_eq!(calls[0].body["eventType"], "Test");
    assert_eq!(calls[0].body["instanceName"], "Radarr");
}

#[tokio::test]
async fn failed_download_is_blocklisted_then_skipped_on_research_then_cleared() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();
    let download_dir = tmp.path().join("downloads");
    std::fs::create_dir_all(&download_dir).unwrap();
    let good_file = download_dir.join("The.Matrix.1999.1080p.WEB-DL.x264-GOOD.mkv");
    std::fs::write(&good_file, b"good").unwrap();

    let node = seed_movie_node(&db, "The Matrix").await;
    let registry = movie_registry(&node, "The Matrix");
    let config = runner_config(library_root.clone());

    // --- First run: the only candidate's download FAILS -> blocklisted. -----
    let bad_release = movie_release("The.Matrix.1999.1080p.BluRay.x264-BAD", "guid-bad");
    {
        let indexer = FakeIndexer {
            releases: vec![bad_release.clone()],
        };
        let client = FakeDownloadClient { content_path: None }; // fails
        let clock = LogicalClock::new(0);
        let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
        let outcome = runner.run(&node).await.unwrap();
        assert!(matches!(outcome, RunOutcome::Failed { .. }), "{outcome:?}");
    }

    // The failed release is now blocklisted for this content.
    assert!(
        BlocklistRepository::is_blocklisted(&db.blocklist(), node.id, &bad_release)
            .await
            .unwrap(),
        "the failed release must be blocklisted"
    );
    let entries = BlocklistRepository::list(&db.blocklist()).await.unwrap();
    assert_eq!(entries.len(), 1);

    // --- Second run (re-search): the blocklisted bad release is offered FIRST,
    // then a good release. The runner must SKIP the blocklisted one and grab the
    // good one. ---------------------------------------------------------------
    let good_release = movie_release("The.Matrix.1999.1080p.WEB-DL.x264-GOOD", "guid-good");
    {
        let indexer = FakeIndexer {
            releases: vec![bad_release.clone(), good_release.clone()],
        };
        let client = FakeDownloadClient {
            content_path: Some(good_file.to_string_lossy().into_owned()),
        };
        let clock = LogicalClock::new(0);
        let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
        let outcome = runner.run(&node).await.unwrap();
        let grab_id = match outcome {
            RunOutcome::Imported { grab_id, .. } => grab_id,
            other => panic!("expected the good release to import, got {other:?}"),
        };
        // The grab that won is the GOOD release, not the blocklisted one.
        let grab = GrabRepository::get(&db.grabs(), grab_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(grab.request.release.guid.as_deref(), Some("guid-good"));
        assert_eq!(grab.status, GrabStatus::Imported);
    }

    // --- Manual remove clears the blocklist; the release is grabbable again. --
    let id = &entries[0].id;
    assert!(BlocklistRepository::remove(&db.blocklist(), id)
        .await
        .unwrap());
    assert!(
        !BlocklistRepository::is_blocklisted(&db.blocklist(), node.id, &bad_release)
            .await
            .unwrap(),
        "after manual remove the release is no longer blocklisted"
    );
    assert!(BlocklistRepository::list(&db.blocklist())
        .await
        .unwrap()
        .is_empty());
}

/// THE SELF-HEAL PROOF: a SINGLE run, two candidates. The first release's
/// download fails and never completes; the runner must blocklist it, remove it
/// from the client (with its local data), grab the SECOND release in the same
/// run, and IMPORT the file. No second manual run, no human intervention.
#[tokio::test]
async fn one_run_self_heals_past_a_failed_release_grabs_the_next_and_imports() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();
    let download_dir = tmp.path().join("downloads");
    std::fs::create_dir_all(&download_dir).unwrap();
    // The GOOD release's download "completes" with this real on-disk file.
    let good_file = download_dir.join("The.Matrix.1999.1080p.WEB-DL.x264-GOOD.mkv");
    std::fs::write(&good_file, b"good bytes").unwrap();

    let node = seed_movie_node(&db, "The Matrix").await;
    let registry = movie_registry(&node, "The Matrix");
    let config = runner_config(library_root.clone());

    // TWO candidates offered in one search: the BAD one first (so it is decided
    // and grabbed first), the GOOD one second.
    let bad = movie_release("The.Matrix.1999.1080p.BluRay.x264-BAD", "guid-bad");
    let good = movie_release("The.Matrix.1999.1080p.WEB-DL.x264-GOOD", "guid-good");
    let indexer = FakeIndexer {
        releases: vec![bad.clone(), good.clone()],
    };
    let removed = Arc::new(Mutex::new(Vec::new()));
    let client = SelfHealClient {
        bad_guid: "guid-bad".into(),
        good_path: good_file.to_string_lossy().into_owned(),
        removed: Arc::clone(&removed),
    };
    let clock = LogicalClock::new(0);
    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);

    // ONE run.
    let outcome = runner.run(&node).await.unwrap();
    let grab_id = match outcome {
        RunOutcome::Imported { grab_id, .. } => grab_id,
        other => panic!("self-heal must end in an import in one run, got {other:?}"),
    };

    // The grab that won is the GOOD release.
    let grab = GrabRepository::get(&db.grabs(), grab_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grab.request.release.guid.as_deref(), Some("guid-good"));
    assert_eq!(grab.status, GrabStatus::Imported);

    // The BAD release was blocklisted (so a future search skips it too)...
    assert!(
        BlocklistRepository::is_blocklisted(&db.blocklist(), node.id, &bad)
            .await
            .unwrap(),
        "the failed first release must be blocklisted"
    );
    // ...and the GOOD release is NOT blocklisted.
    assert!(
        !BlocklistRepository::is_blocklisted(&db.blocklist(), node.id, &good)
            .await
            .unwrap()
    );

    // The dead download was removed from the client WITH its local data.
    let removed = removed.lock().unwrap().clone();
    assert_eq!(removed.len(), 1, "exactly the failed download was removed");
    assert_eq!(
        removed[0],
        ("dl::guid-bad".to_string(), true),
        "the failed download is removed with delete-local-data"
    );

    // THE PROOF: the GOOD file imported on disk under the library root.
    let imported = find_one_file(&library_root).expect("an imported file must exist");
    assert_eq!(std::fs::read(&imported).unwrap(), b"good bytes");
}

/// A sustained STALL (downloading, no progress, zero peers) — not just a hard
/// `Failed` — drives the same self-heal: blocklist the stalled release, grab the
/// next, import it.
#[tokio::test]
async fn a_stalled_download_self_heals_to_the_next_release() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();
    let download_dir = tmp.path().join("downloads");
    std::fs::create_dir_all(&download_dir).unwrap();
    let good_file = download_dir.join("The.Matrix.1999.1080p.WEB-DL.x264-GOOD.mkv");
    std::fs::write(&good_file, b"good bytes").unwrap();

    let node = seed_movie_node(&db, "The Matrix").await;
    let registry = movie_registry(&node, "The Matrix");
    let config = runner_config(library_root.clone());

    let stalling = movie_release("The.Matrix.1999.1080p.BluRay.x264-STALL", "guid-stall");
    let good = movie_release("The.Matrix.1999.1080p.WEB-DL.x264-GOOD", "guid-good");
    let indexer = FakeIndexer {
        releases: vec![stalling.clone(), good.clone()],
    };

    // First, prove the STALL detector specifically: ONE stalling candidate driven
    // through a client that stays Downloading with no progress and zero peers must
    // fail the run as a stall (not a hard `Failed`).
    let stall_indexer = FakeIndexer {
        releases: vec![stalling.clone()],
    };
    let stall_client = StallingClient;
    let clock = LogicalClock::new(0);
    let runner = PipelineRunner::new(
        &stall_indexer,
        &stall_client,
        &registry,
        &db,
        &clock,
        &config,
    );
    let outcome = runner.run(&node).await.unwrap();
    match outcome {
        RunOutcome::Failed { detail } => assert!(
            detail.contains("stalled"),
            "the failure detail must name the stall, got {detail:?}"
        ),
        other => panic!("a pure stall must fail the run, got {other:?}"),
    }
    // The stalled release is now blocklisted.
    assert!(
        BlocklistRepository::is_blocklisted(&db.blocklist(), node.id, &stalling)
            .await
            .unwrap(),
        "the stalled release must be blocklisted"
    );

    // Now ONE run with both candidates self-heals to the good one.
    let removed2 = Arc::new(Mutex::new(Vec::new()));
    let heal_client = SelfHealClient {
        bad_guid: "guid-stall".into(),
        good_path: good_file.to_string_lossy().into_owned(),
        removed: Arc::clone(&removed2),
    };
    let runner = PipelineRunner::new(&indexer, &heal_client, &registry, &db, &clock, &config);
    let outcome = runner.run(&node).await.unwrap();
    let grab_id = match outcome {
        RunOutcome::Imported { grab_id, .. } => grab_id,
        other => panic!("expected self-heal import, got {other:?}"),
    };
    let grab = GrabRepository::get(&db.grabs(), grab_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grab.request.release.guid.as_deref(), Some("guid-good"));
    assert_eq!(grab.status, GrabStatus::Imported);
    assert!(find_one_file(&library_root).is_some());
}

/// The grab-next loop is BOUNDED: when every candidate fails, the run terminates
/// (it does not loop forever) and ends in `Failed`. With more failing candidates
/// than the attempt cap, the cap is what halts it.
#[tokio::test]
async fn grab_next_retry_loop_halts_when_every_candidate_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();

    let node = seed_movie_node(&db, "The Matrix").await;
    let registry = movie_registry(&node, "The Matrix");
    let config = runner_config(library_root);

    // Ten candidates — more than the grab-next attempt cap — and EVERY one stalls.
    let releases: Vec<Release> = (0..10)
        .map(|i| {
            movie_release(
                &format!("The.Matrix.1999.1080p.x264-BAD{i}"),
                &format!("guid-{i}"),
            )
        })
        .collect();
    let indexer = FakeIndexer {
        releases: releases.clone(),
    };
    let client = StallingClient;
    let clock = LogicalClock::new(0);
    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);

    // The run TERMINATES (the test completing at all is the no-infinite-loop proof)
    // and ends Failed.
    let outcome = runner.run(&node).await.unwrap();
    assert!(
        matches!(outcome, RunOutcome::Failed { .. }),
        "every-candidate-fails must end Failed, got {outcome:?}"
    );

    // The cap bounded the work: at most the attempt-cap number of releases were
    // grabbed-and-blocklisted, never all ten. (The exact const lives in the runner;
    // the contract is simply "fewer than offered, and bounded".)
    let blocklisted = BlocklistRepository::list(&db.blocklist()).await.unwrap();
    assert!(
        !blocklisted.is_empty(),
        "at least one failing release was blocklisted"
    );
    assert!(
        blocklisted.len() < releases.len(),
        "the cap halts the loop before trying every one of the {} candidates (blocklisted {})",
        releases.len(),
        blocklisted.len()
    );
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

/// A near-complete download (≈99.9%, zero peers — a torrent's end-game where the
/// final piece is verifying and seeders have dropped off) must NOT be killed by
/// the stall detector. It finishes and imports. Regression for the live bug where
/// a fully-downloaded torrent was failed as a "stall" moments before import.
#[tokio::test]
async fn a_near_complete_download_is_not_killed_by_the_stall_detector() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("c.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();
    let download_dir = tmp.path().join("downloads");
    std::fs::create_dir_all(&download_dir).unwrap();
    let file = download_dir.join("The.Matrix.1999.1080p.WEB-DL.x264-GOOD.mkv");
    std::fs::write(&file, b"good bytes").unwrap();

    let node = seed_movie_node(&db, "The Matrix").await;
    let registry = movie_registry(&node, "The Matrix");
    // Give the poll budget headroom past the end-game window.
    let mut config = runner_config(library_root.clone());
    config.max_track_polls = 10;

    let release = movie_release("The.Matrix.1999.1080p.WEB-DL.x264-GOOD", "guid-good");
    let indexer = FakeIndexer {
        releases: vec![release.clone()],
    };
    let client = EndgameClient {
        completed_path: file.to_string_lossy().into_owned(),
        polls: std::sync::atomic::AtomicU32::new(0),
    };
    let clock = LogicalClock::new(0);
    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
    let outcome = runner.run(&node).await.unwrap();
    let grab_id = match outcome {
        RunOutcome::Imported { grab_id, .. } => grab_id,
        other => panic!("a near-complete download must import, not stall-fail: {other:?}"),
    };
    let grab = GrabRepository::get(&db.grabs(), grab_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grab.status, GrabStatus::Imported);
    assert!(
        find_one_file(&library_root).is_some(),
        "the imported file must exist in the library"
    );
    // A successful download is never blocklisted (no self-heal needed).
    assert!(
        !BlocklistRepository::is_blocklisted(&db.blocklist(), node.id, &release)
            .await
            .unwrap(),
        "a successful near-complete download must not be blocklisted"
    );
}
