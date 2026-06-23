//! Shared test harness for the API integration tests.
//!
//! Each test gets a fresh in-memory database (naturally isolated, no temp file
//! needed for the DB itself) and a server bound to `127.0.0.1:0` — the OS picks
//! the port, which the test reads back, so nothing is hardcoded and N tests run
//! in parallel without collision (docs/16-local-dev-and-testing.md).

#![allow(dead_code)]

use cellarr_api::{AppState, AuthConfig};
use cellarr_core::{
    DownloadClientConfig, IndexerConfig, Library, LibraryId, MediaType, Protocol, QualityProfile,
    QualityProfileId,
};
use cellarr_db::Database;

/// The API key used by tests that exercise auth.
pub const TEST_API_KEY: &str = "test-key-not-a-real-secret";

/// A running test server: its base URL and the state behind it.
///
/// Holds the [`tempfile::TempDir`] for the SQLite file so the data dir lives
/// exactly as long as the server and is cleaned up on drop. The in-memory engine
/// pins a single connection that the writer-actor holds for life, which would
/// starve concurrent reads under the server; a file-backed DB (8-connection
/// pool) is the documented test shape (docs/16-local-dev-and-testing.md).
pub struct TestServer {
    pub base_url: String,
    pub state: AppState,
    _dir: tempfile::TempDir,
    _handle: tokio::task::JoinHandle<()>,
}

/// Spin up a server with auth disabled (zero-config first-run behavior).
pub async fn start_open() -> TestServer {
    start_with(AuthConfig::disabled()).await
}

/// Spin up a server requiring [`TEST_API_KEY`] on mutating endpoints.
pub async fn start_authed() -> TestServer {
    start_with(AuthConfig::with_key(TEST_API_KEY)).await
}

async fn start_with(auth: AuthConfig) -> TestServer {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("cellarr-test.db");
    let db = Database::open(db_path.to_str().expect("utf-8 path"))
        .await
        .expect("open file db");
    let state = AppState::new(db, auth);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    let base_url = format!("http://{addr}");

    let serve_state = state.clone();
    let handle = tokio::spawn(async move {
        // The server runs until the test process exits; errors here only matter
        // if a test is still talking to it, in which case the request fails and
        // the assertion catches it.
        let _ = cellarr_api::serve(listener, serve_state).await;
    });

    TestServer {
        base_url,
        state,
        _dir: dir,
        _handle: handle,
    }
}

impl TestServer {
    /// A reqwest client. Each test builds its own.
    pub fn client(&self) -> reqwest::Client {
        reqwest::Client::new()
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}

// --- fixture builders ------------------------------------------------------

/// Insert a quality profile and return its id.
pub async fn seed_profile(state: &AppState, name: &str) -> QualityProfileId {
    let profile = QualityProfile {
        id: QualityProfileId::new(),
        name: name.to_string(),
        allowed_qualities: vec![1, 2, 3],
        upgrades_allowed: true,
        cutoff_quality: 3,
        min_custom_format_score: 0,
        upgrade_until_custom_format_score: 100,
        required_languages: vec![],
    };
    let id = profile.id;
    state
        .db
        .profiles()
        .upsert_profile(&profile)
        .await
        .expect("seed profile");
    id
}

/// Insert a library of the given media type, with a fresh default profile.
pub async fn seed_library(state: &AppState, media_type: MediaType, name: &str) -> LibraryId {
    let profile_id = seed_profile(state, &format!("{name}-profile")).await;
    let library = Library {
        id: LibraryId::new(),
        media_type,
        name: name.to_string(),
        root_folders: vec!["/data".to_string()],
        default_quality_profile: profile_id,
    };
    let id = library.id;
    state
        .db
        .config()
        .upsert_library(&library)
        .await
        .expect("seed library");
    id
}

/// Insert an indexer config and return it.
pub async fn seed_indexer(state: &AppState, name: &str) -> IndexerConfig {
    let indexer = IndexerConfig {
        id: cellarr_core::IndexerId::new(),
        name: name.to_string(),
        kind: "torznab".to_string(),
        protocol: Protocol::Torrent,
        enabled: true,
        priority: 25,
        settings: serde_json::json!({ "baseUrl": "http://example.invalid" }),
    };
    state
        .db
        .config()
        .upsert_indexer(&indexer)
        .await
        .expect("seed indexer");
    indexer
}

/// Insert a download client config and return it.
pub async fn seed_download_client(state: &AppState, name: &str) -> DownloadClientConfig {
    let client = DownloadClientConfig {
        id: cellarr_core::DownloadClientId::new(),
        name: name.to_string(),
        kind: "qbittorrent".to_string(),
        protocol: Protocol::Torrent,
        enabled: true,
        priority: 1,
        category: "cellarr".to_string(),
        settings: serde_json::json!({ "host": "127.0.0.1" }),
    };
    state
        .db
        .config()
        .upsert_download_client(&client)
        .await
        .expect("seed download client");
    client
}
