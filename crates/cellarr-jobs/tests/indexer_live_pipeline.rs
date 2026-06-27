//! LIVE end-to-end discovery test.
//!
//! Unlike `pipeline_e2e.rs` (which uses a hand-built FAKE indexer), this test
//! exercises the *real* live path:
//!
//!   1. A tiny **local HTTP server** (bound to `127.0.0.1` on an OS-allocated
//!      port) that speaks Torznab — `?t=caps` returns a caps XML, `?t=tvsearch`
//!      returns an RSS feed with several realistic scene releases.
//!   2. The indexer is **configured through the db `ConfigRepo`** exactly as the
//!      `/api/v3/indexer` CRUD path persists it (baseUrl + apiKey in `settings`).
//!   3. [`DbIndexerSet`] reads that config, builds the native [`TorznabIndexer`]
//!      adapter, and is handed to the **real `PipelineRunner`**, which makes
//!      genuine HTTP requests to the local server: `t=caps` first, then the typed
//!      search.
//!   4. We assert the releases are discovered, parsed (title/quality), and the
//!      best is decided + grabbed + imported to disk.
//!
//! A second test covers the **error path**: an indexer whose server answers
//! `t=caps` with `401 Unauthorized` discovers nothing, and the run ends in the
//! logged "no releases" outcome rather than panicking.
//!
//! No real tracker is contacted — the only network is loopback to our own server.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use cellarr_core::{
    repo::{ContentRepository, GrabRepository, HistoryRepository},
    ContentId, ContentRef, Coordinates, CustomFormat, DownloadClientId, GrabStatus, IndexerConfig,
    IndexerId, Library, LibraryId, MediaType, Protocol, QualityProfile, QualityProfileId,
    QualityRanking,
};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_jobs::clock::LogicalClock;
use cellarr_jobs::runner::{PipelineRunner, RunOutcome, RunnerConfig};
use cellarr_jobs::DbIndexerSet;
use cellarr_media::{
    ContentCandidate, ContentLookup, MediaRegistry, MetadataLookup, MovieMeta, SeriesMeta, TvModule,
};
use serde_json::json;

// ---------------------------------------------------------------------------
// A minimal local Torznab HTTP server.
// ---------------------------------------------------------------------------

/// What the test server should answer `t=caps` with.
#[derive(Clone, Copy)]
enum CapsBehavior {
    /// 200 with a valid caps document.
    Ok,
    /// 401 Unauthorized (a banned / wrong API key).
    Unauthorized,
}

/// A live local Torznab server. Holds the bound address and the serving task;
/// dropping it aborts the task.
struct MockTorznab {
    addr: std::net::SocketAddr,
    handle: JoinHandle<()>,
}

impl Drop for MockTorznab {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl MockTorznab {
    /// Bind to an ephemeral loopback port and start serving Torznab.
    async fn start(caps: CapsBehavior) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    // First line: "GET /api?t=caps&... HTTP/1.1"
                    let target = req.lines().next().unwrap_or("").to_string();
                    let response = route(&target, caps);
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });
        MockTorznab { addr, handle }
    }

    /// The base URL an indexer config should point at.
    fn base_url(&self) -> String {
        format!("http://{}/api", self.addr)
    }
}

/// Route one request line to a full HTTP/1.1 response.
fn route(request_line: &str, caps: CapsBehavior) -> String {
    if request_line.contains("t=caps") {
        return match caps {
            CapsBehavior::Ok => http_ok("application/xml", CAPS_XML),
            CapsBehavior::Unauthorized => {
                http_status(401, "Unauthorized", "text/plain", "bad apikey")
            }
        };
    }
    if request_line.contains("t=tvsearch") || request_line.contains("t=search") {
        return http_ok("application/rss+xml", SEARCH_RSS);
    }
    http_status(404, "Not Found", "text/plain", "unknown mode")
}

fn http_ok(content_type: &str, body: &str) -> String {
    http_status(200, "OK", content_type, body)
}

fn http_status(code: u16, reason: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        len = body.len(),
    )
}

/// Synthetic Torznab caps: advertises search + tvsearch with the params the
/// adapter will send for a TV query.
const CAPS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<caps>
  <server version="1.1" title="Mock Torznab" />
  <limits max="100" default="50" />
  <searching>
    <search available="yes" supportedParams="q" />
    <tv-search available="yes" supportedParams="q,season,ep,tvdbid" />
    <movie-search available="no" supportedParams="q" />
  </searching>
  <categories>
    <category id="5000" name="TV">
      <subcat id="5040" name="TV/HD" />
    </category>
  </categories>
</caps>"#;

/// Synthetic Torznab search RSS: three realistic scene releases for a TV episode
/// at different qualities, with torznab attrs (size, seeders, freeleech).
const SEARCH_RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">
  <channel>
    <title>Mock Torznab</title>
    <item>
      <title>The.Expanse.S02E05.1080p.BluRay.x264-DEMAND</title>
      <guid isPermaLink="false">https://mock.local/details?id=501</guid>
      <link>https://mock.local/details?id=501</link>
      <enclosure url="magnet:?xt=urn:btih:1111111111111111111111111111111111111111&amp;dn=The.Expanse.S02E05.1080p" length="3221225472" type="application/x-bittorrent" />
      <torznab:attr name="category" value="5040" />
      <torznab:attr name="size" value="3221225472" />
      <torznab:attr name="seeders" value="210" />
      <torznab:attr name="downloadvolumefactor" value="0" />
    </item>
    <item>
      <title>The.Expanse.S02E05.720p.WEB-DL.DD5.1.H.264-NTb</title>
      <guid isPermaLink="false">https://mock.local/details?id=502</guid>
      <link>https://mock.local/details?id=502</link>
      <enclosure url="https://mock.local/download/502.torrent" length="1610612736" type="application/x-bittorrent" />
      <torznab:attr name="category" value="5040" />
      <torznab:attr name="size" value="1610612736" />
      <torznab:attr name="seeders" value="58" />
      <torznab:attr name="downloadvolumefactor" value="1" />
    </item>
    <item>
      <title>The.Expanse.S02E05.HDTV.x264-LOL</title>
      <guid isPermaLink="false">https://mock.local/details?id=503</guid>
      <link>https://mock.local/details?id=503</link>
      <enclosure url="https://mock.local/download/503.torrent" length="524288000" type="application/x-bittorrent" />
      <torznab:attr name="category" value="5040" />
      <torznab:attr name="size" value="524288000" />
      <torznab:attr name="seeders" value="12" />
    </item>
  </channel>
</rss>"#;

// ---------------------------------------------------------------------------
// Synthetic pipeline seams (offline; same pattern as pipeline_e2e.rs).
// ---------------------------------------------------------------------------

/// A FAKE download client that immediately "completes" with a real temp file.
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
        Ok(None)
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

/// Persist a Torznab indexer exactly as the `/api/v3/indexer` shim does: identity
/// + a `settings` JSON object carrying baseUrl / apiKey / categories.
async fn configure_indexer(db: &Database, base_url: &str) -> IndexerConfig {
    let config = IndexerConfig {
        tags: Vec::new(),
        id: IndexerId::new(),
        name: "Mock Torznab".into(),
        kind: "torznab".into(),
        protocol: Protocol::Torrent,
        enabled: true,
        priority: 25,
        criteria: Default::default(),
        settings: json!({
            "baseUrl": base_url,
            "apiKey": "test-key",
            "categories": [5040],
        }),
    };
    db.config().upsert_indexer(&config).await.unwrap();
    config
}

async fn seed_tv_episode(db: &Database, season: u32, episode: u32) -> ContentRef {
    let library_id = LibraryId::new();
    let library = Library {
        id: library_id,
        media_type: MediaType::Tv,
        name: "TV lib".into(),
        root_folders: vec!["/tmp/synthetic".into()],
        default_quality_profile: QualityProfileId::new(),
    };
    db.config().upsert_library(&library).await.unwrap();

    let content_id = ContentId::new();
    let coords = Coordinates::Episode {
        season,
        episode,
        absolute: None,
    };
    let node = cellarr_core::ContentNode {
        tags: Vec::new(),
        id: content_id,
        library_id,
        media_type: MediaType::Tv,
        parent_id: None,
        kind: cellarr_core::ContentKind::Episode,
        series_type: cellarr_core::SeriesType::Standard,
        coords: coords.clone(),
        monitored: true,
        title_id: None,
    };
    db.content().upsert(&node).await.unwrap();
    ContentRef::new(content_id, library_id, MediaType::Tv, coords).unwrap()
}

fn tv_registry(node: &ContentRef, title: &str) -> MediaRegistry {
    let candidate = ContentCandidate {
        content_ref: node.clone(),
        title: title.to_string(),
        aliases: Vec::new(),
    };
    let mut registry = MediaRegistry::new();
    registry.register(TvModule::new(
        MockContentLookup { candidate },
        MockMetadata {
            series: Some(SeriesMeta {
                title: title.to_string(),
                aliases: Vec::new(),
                year: Some(2017),
                external_ids: Vec::new(),
            }),
        },
    ));
    registry
}

fn runner_config(library_root: PathBuf) -> RunnerConfig {
    RunnerConfig {
        content_tag_ids: Vec::new(),
        profile: permissive_profile(),
        custom_formats: Vec::<CustomFormat>::new(),
        ranking: QualityRanking::default(),
        proper_repack_policy: ProperRepackPolicy::default(),
        library_root,
        naming_format: "{Series Title}/{Series Title}.{Extension}".into(),
        anime_naming_format: String::new(),
        series_type: cellarr_core::SeriesType::Standard,
        indexer_id: IndexerId::new(),
        client_id: DownloadClientId::new(),
        category: "cellarr".into(),
        max_track_polls: 5,
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
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn live_torznab_search_drives_discover_through_decide_to_import() {
    // 1. Stand up the live local Torznab server.
    let server = MockTorznab::start(CapsBehavior::Ok).await;

    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();

    // A real "downloaded" file the fake client will point at on completion.
    let download_dir = tmp.path().join("downloads");
    std::fs::create_dir_all(&download_dir).unwrap();
    let downloaded = download_dir.join("The.Expanse.S02E05.1080p.BluRay.x264-DEMAND.mkv");
    std::fs::write(&downloaded, b"synthetic episode bytes").unwrap();

    let library_root = tmp.path().join("library/tv");
    std::fs::create_dir_all(&library_root).unwrap();

    // 2. Configure the indexer via the db (as the API CRUD would).
    let configured = configure_indexer(&db, &server.base_url()).await;

    // Sanity: it persisted and is visible as enabled.
    let enabled = db.config().list_enabled_indexers().await.unwrap();
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].id, configured.id);

    let node = seed_tv_episode(&db, 2, 5).await;
    let registry = tv_registry(&node, "The Expanse");

    // 3. The LIVE aggregate indexer over the db config + the real runner.
    let indexer = DbIndexerSet::new(db.clone());
    let client = FakeDownloadClient {
        content_path: downloaded.to_string_lossy().into_owned(),
    };
    let clock = LogicalClock::new(0);
    let config = runner_config(library_root.clone());
    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);

    let outcome = runner.run(&node).await.unwrap();

    // The best discovered release (1080p BluRay) was decided + grabbed + imported.
    let (grab_id, destinations) = match outcome {
        RunOutcome::Imported {
            grab_id,
            destinations,
        } => (grab_id, destinations),
        other => panic!("expected Imported from live search, got {other:?}"),
    };
    assert_eq!(destinations.len(), 1);
    let dest = PathBuf::from(&destinations[0]);
    assert!(dest.exists(), "imported file must exist at {dest:?}");
    assert!(dest.starts_with(&library_root));

    let grab = GrabRepository::get(&db.grabs(), grab_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grab.status, GrabStatus::Imported);

    // The grabbed release is the parsed 1080p BluRay scene title from the FEED —
    // proving the live HTTP search results flowed through parse -> decide -> grab.
    assert_eq!(
        grab.request.release.title,
        "The.Expanse.S02E05.1080p.BluRay.x264-DEMAND"
    );
    assert!(
        grab.request.release.download_url.starts_with("magnet:?"),
        "download url came from the feed enclosure: {}",
        grab.request.release.download_url
    );
    assert!(
        grab.request
            .release
            .indexer_flags
            .contains(&"freeleech".to_string()),
        "freeleech flag parsed from torznab attr: {:?}",
        grab.request.release.indexer_flags
    );
    assert_eq!(grab.request.release.seeders, Some(210));

    // History + decision log explain the grab.
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
    let run_id = history
        .iter()
        .find_map(|h| match h.event {
            cellarr_core::history::HistoryEvent::Grabbed { .. } => Some(h.run_id),
            _ => None,
        })
        .unwrap();
    let records = db.decision_log().for_run(run_id).await.unwrap();
    assert!(records.iter().any(|r| matches!(
        r.decision.as_ref().map(|d| &d.verdict),
        Some(cellarr_core::Verdict::Grab { .. })
    )));
}

#[tokio::test]
async fn live_torznab_search_directly_returns_all_parsed_releases() {
    // A lower-level assertion on the live seam alone: every feed item is
    // discovered and normalized into a Release with the parsed torznab attrs.
    let server = MockTorznab::start(CapsBehavior::Ok).await;
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();
    configure_indexer(&db, &server.base_url()).await;

    let indexer = DbIndexerSet::new(db.clone());
    let terms = cellarr_core::SearchTerms {
        queries: vec!["The Expanse".into()],
        ids: vec![("tvdbid".into(), "121361".into())],
        numbering: vec![("season".into(), "2".into()), ("ep".into(), "5".into())],
    };
    use cellarr_core::traits::Indexer;
    let releases = indexer.search(&terms).await.unwrap();

    assert_eq!(releases.len(), 3, "all three feed items discovered");
    let titles: Vec<&str> = releases.iter().map(|r| r.title.as_str()).collect();
    assert!(titles.contains(&"The.Expanse.S02E05.1080p.BluRay.x264-DEMAND"));
    assert!(titles.contains(&"The.Expanse.S02E05.720p.WEB-DL.DD5.1.H.264-NTb"));
    assert!(titles.contains(&"The.Expanse.S02E05.HDTV.x264-LOL"));

    // Parse each title to prove the quality is recoverable downstream.
    let parsed = cellarr_parse::parse_title(&releases[0].title);
    assert!(
        format!("{:?}", parsed).to_lowercase().contains("1080"),
        "1080p quality parsed from the live release title: {parsed:?}"
    );
}

#[tokio::test]
async fn live_torznab_unauthorized_caps_discovers_nothing() {
    // The error path: the server rejects t=caps with 401. The caps-first adapter
    // surfaces it; DbIndexerSet skips the failing indexer (best-effort fan-out),
    // so the run finds no releases and ends in the logged "nothing found"
    // outcome rather than erroring out.
    let server = MockTorznab::start(CapsBehavior::Unauthorized).await;
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();
    configure_indexer(&db, &server.base_url()).await;

    let node = seed_tv_episode(&db, 2, 5).await;
    let registry = tv_registry(&node, "The Expanse");

    let indexer = DbIndexerSet::new(db.clone());
    let client = FakeDownloadClient {
        content_path: "/dev/null".into(),
    };
    let clock = LogicalClock::new(0);
    let config = runner_config(tmp.path().join("library/tv"));
    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);

    let outcome = runner.run(&node).await.unwrap();
    assert!(
        matches!(outcome, RunOutcome::NothingFound),
        "401 caps yields no releases -> NothingFound, got {outcome:?}"
    );
}

#[tokio::test]
async fn live_torznab_fail_fast_surfaces_the_401() {
    // With fail_fast=true the same 401 is surfaced as a hard search error rather
    // than silently skipped — proving the adapter's caps-first 401 propagates.
    let server = MockTorznab::start(CapsBehavior::Unauthorized).await;
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();
    configure_indexer(&db, &server.base_url()).await;

    let indexer = DbIndexerSet::with_rate_limiter(
        db.clone(),
        Arc::new(cellarr_indexers::HostRateLimiter::conservative_default()),
        /* fail_fast = */ true,
    );
    use cellarr_core::traits::Indexer;
    let terms = cellarr_core::SearchTerms {
        queries: vec!["The Expanse".into()],
        ids: vec![],
        numbering: vec![],
    };
    let err = indexer.search(&terms).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("401") && msg.contains("Mock Torznab"),
        "fail-fast surfaces the named indexer's 401: {msg}"
    );
}
