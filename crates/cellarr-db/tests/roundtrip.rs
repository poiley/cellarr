//! Integration tests for cellarr-db against a tempfile SQLite database.
//!
//! These exercise the real on-disk path: open + migrate, the writer-actor for
//! every mutation, the repository trait implementations, and FTS search. A
//! per-test `tempfile::TempDir` keeps runs hermetic and isolated.

use cellarr_core::decision::{Score, Verdict};
use cellarr_core::history::{DecisionLogRecord, HistoryEvent, HistoryRecord};
use cellarr_core::pipeline::{Stage, Transition, TransitionKind};
use cellarr_core::profile::Quality;
use cellarr_core::repo::{
    ContentRepository, DecisionLogRepository, GrabRepository, HistoryRepository,
    MediaFileRepository, ProfileRepository,
};
use cellarr_core::{
    Condition, ConditionKind, ContentId, ContentKind, ContentNode, ContentRef, Coordinates,
    CustomFormat, CustomFormatId, DownloadClientConfig, DownloadClientId, Grab, GrabRequest,
    GrabStatus, IndexerConfig, IndexerId, Library, LibraryId, MediaFile, MediaFileId, MediaType,
    NotificationConfig, PipelineRunId, Protocol, QualityProfile, QualityProfileId, Release,
    RootFolder, Source,
};
use cellarr_db::Database;
use tempfile::TempDir;
use time::OffsetDateTime;

/// Open a fresh migrated database under a temp dir; returns the dir so it lives
/// for the test's duration.
async fn temp_db() -> (TempDir, Database) {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("cellarr.db");
    let db = Database::open(path.to_str().expect("utf8 path"))
        .await
        .expect("open + migrate");
    (dir, db)
}

fn movie_node(library_id: LibraryId, id: ContentId) -> ContentNode {
    ContentNode {
        id,
        library_id,
        media_type: MediaType::Movie,
        parent_id: None,
        kind: ContentKind::Movie,
        coords: Coordinates::Movie,
        monitored: true,
        title_id: None,
    }
}

#[tokio::test]
async fn open_runs_migrations_and_creates_tables() {
    let (_dir, db) = temp_db().await;
    // Querying a known table proves the migration ran.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM content")
        .fetch_one(db.pool())
        .await
        .expect("content table exists");
    assert_eq!(count, 0);

    // The typed identity side-tables and the FTS table exist too.
    for table in ["movie_meta", "series_meta", "episode_meta", "cache"] {
        let q = format!("SELECT COUNT(*) FROM {table}");
        let n: i64 = sqlx::query_scalar(&q)
            .fetch_one(db.pool())
            .await
            .unwrap_or_else(|e| panic!("table {table} should exist: {e}"));
        assert_eq!(n, 0);
    }
}

#[tokio::test]
async fn library_and_content_round_trip_via_writer() {
    let (_dir, db) = temp_db().await;
    let config = db.config();
    let content = db.content();

    let library = Library {
        id: LibraryId::new(),
        media_type: MediaType::Movie,
        name: "Movies — 4K".to_string(),
        root_folders: vec!["/data/movies".to_string(), "/data/movies-4k".to_string()],
        default_quality_profile: QualityProfileId::new(),
    };
    config
        .upsert_library(&library)
        .await
        .expect("upsert library");

    let fetched = config
        .get_library(library.id)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(fetched, library);

    let node = movie_node(library.id, ContentId::new());
    content.upsert(&node).await.expect("upsert content");

    let got: Option<ContentRef> = content.get(node.id).await.expect("get content");
    assert_eq!(got, Some(node.as_ref()));
}

#[tokio::test]
async fn monitored_missing_excludes_nodes_with_files_and_containers() {
    let (_dir, db) = temp_db().await;
    let config = db.config();
    let content = db.content();

    let library = Library {
        id: LibraryId::new(),
        media_type: MediaType::Movie,
        name: "Movies".to_string(),
        root_folders: vec!["/data".to_string()],
        default_quality_profile: QualityProfileId::new(),
    };
    config.upsert_library(&library).await.unwrap();

    // A monitored, file-less movie -> missing.
    let missing = movie_node(library.id, ContentId::new());
    content.upsert(&missing).await.unwrap();

    // A monitored movie that already has a file -> not missing.
    let have = movie_node(library.id, ContentId::new());
    content.upsert(&have).await.unwrap();
    let media_files = db.media_files();
    let file = MediaFile {
        id: MediaFileId::new(),
        path: "/data/have.mkv".to_string(),
        size: 100,
        quality: Quality::new("Bluray-1080p", 14),
        languages: vec![],
        media_info: None,
        custom_format_score: None,
        release_type: None,
    };
    media_files.create(&file).await.expect("create file");
    media_files.link(have.id, file.id).await.expect("link file");

    let result = content.monitored_missing().await.expect("query");
    let ids: Vec<ContentId> = result.iter().map(|r| r.id).collect();
    assert!(
        ids.contains(&missing.id),
        "file-less movie should be missing"
    );
    assert!(
        !ids.contains(&have.id),
        "movie with a linked file is not missing"
    );
}

#[tokio::test]
async fn grab_create_and_get_round_trip() {
    let (_dir, db) = temp_db().await;
    let grabs = db.grabs();

    let content_ref = ContentRef {
        id: ContentId::new(),
        library_id: LibraryId::new(),
        media_type: MediaType::Movie,
        coords: Coordinates::Movie,
    };
    let release = Release {
        indexer_id: IndexerId::new(),
        title: "Some.Movie.2024.1080p.BluRay.x264-GROUP".to_string(),
        download_url: "magnet:?xt=urn:btih:abc".to_string(),
        guid: Some("guid-123".to_string()),
        protocol: Protocol::Torrent,
        size: Some(8_000_000_000),
        seeders: Some(42),
        indexer_flags: vec!["freeleech".to_string()],
    };
    let request = GrabRequest {
        content_ref,
        release,
        indexer_id: IndexerId::new(),
        client_id: DownloadClientId::new(),
        category: "cellarr-movies".to_string(),
        // The durable release type round-trips through the new grab column
        // (asserted by the full `fetched == request` equality below).
        release_type: Some(cellarr_core::ReleaseType::Movie),
    };

    let id = grabs.create(&request).await.expect("create grab");
    let fetched = grabs.get(id).await.expect("get").expect("present");
    // A freshly created grab is the request plus its initial lifecycle state.
    assert_eq!(
        fetched,
        Grab {
            id,
            request: request.clone(),
            download_id: None,
            status: GrabStatus::Pending,
        }
    );

    assert!(grabs
        .get(cellarr_core::GrabId::new())
        .await
        .expect("get missing")
        .is_none());
}

#[tokio::test]
async fn grab_status_transitions_and_download_id() {
    let (_dir, db) = temp_db().await;
    let grabs = db.grabs();

    let request = GrabRequest {
        content_ref: ContentRef {
            id: ContentId::new(),
            library_id: LibraryId::new(),
            media_type: MediaType::Movie,
            coords: Coordinates::Movie,
        },
        release: Release {
            indexer_id: IndexerId::new(),
            title: "Movie.2024.2160p.WEB-DL-G".to_string(),
            download_url: "https://nzb/x".to_string(),
            guid: Some("g1".to_string()),
            protocol: Protocol::Usenet,
            size: Some(20_000_000_000),
            seeders: None,
            indexer_flags: vec![],
        },
        indexer_id: IndexerId::new(),
        client_id: DownloadClientId::new(),
        category: "cellarr-movies".to_string(),
        release_type: None,
    };
    let id = grabs.create(&request).await.expect("create");

    // Record the download client's id, then walk the lifecycle.
    grabs
        .set_download_id(id, "sab-nzo-42")
        .await
        .expect("set download id");
    grabs
        .set_status(id, GrabStatus::Downloading)
        .await
        .expect("downloading");
    let g = grabs.get(id).await.expect("get").expect("present");
    assert_eq!(g.download_id.as_deref(), Some("sab-nzo-42"));
    assert_eq!(g.status, GrabStatus::Downloading);

    grabs
        .set_status(id, GrabStatus::Imported)
        .await
        .expect("imported");
    let g = grabs.get(id).await.expect("get").expect("present");
    assert_eq!(g.status, GrabStatus::Imported);
    // The download id survives later status changes.
    assert_eq!(g.download_id.as_deref(), Some("sab-nzo-42"));
}

#[tokio::test]
async fn media_file_create_get_and_list_for_content() {
    let (_dir, db) = temp_db().await;
    let config = db.config();
    let content = db.content();
    let media_files = db.media_files();

    let library = Library {
        id: LibraryId::new(),
        media_type: MediaType::Tv,
        name: "TV".to_string(),
        root_folders: vec!["/tv".to_string()],
        default_quality_profile: QualityProfileId::new(),
    };
    config.upsert_library(&library).await.unwrap();

    // Two episode nodes that a single multi-episode file satisfies.
    let mut ep1 = movie_node(library.id, ContentId::new());
    ep1.media_type = MediaType::Tv;
    ep1.kind = ContentKind::Episode;
    ep1.coords = Coordinates::Episode {
        season: 1,
        episode: 1,
        absolute: None,
    };
    let mut ep2 = ContentNode {
        id: ContentId::new(),
        coords: Coordinates::Episode {
            season: 1,
            episode: 2,
            absolute: None,
        },
        ..ep1.clone()
    };
    ep2.id = ContentId::new();
    content.upsert(&ep1).await.unwrap();
    content.upsert(&ep2).await.unwrap();

    let file = MediaFile {
        id: MediaFileId::new(),
        path: "/tv/show/S01E01-E02.mkv".to_string(),
        size: 3_000_000_000,
        quality: Quality::new("WEBDL-1080p", 13),
        languages: vec!["en".to_string(), "ja".to_string()],
        media_info: Some(serde_json::json!({"video": "h264", "runtime": 1320})),
        custom_format_score: Some(25),
        // A multi-episode file; the durable release type round-trips through the
        // new column (asserted by the `got == file` equality below).
        release_type: Some(cellarr_core::ReleaseType::MultiEpisode),
    };
    media_files.create(&file).await.expect("create");

    let got = media_files
        .get(file.id)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(got, file);

    // One file linked to both episodes -> list_for_content returns it for each.
    media_files.link(ep1.id, file.id).await.expect("link ep1");
    media_files.link(ep2.id, file.id).await.expect("link ep2");
    // Re-linking is idempotent (no duplicate edge / error).
    media_files.link(ep1.id, file.id).await.expect("relink ep1");

    let for_ep1 = media_files
        .list_for_content(ep1.id)
        .await
        .expect("list ep1");
    assert_eq!(for_ep1, vec![file.clone()]);
    let for_ep2 = media_files
        .list_for_content(ep2.id)
        .await
        .expect("list ep2");
    assert_eq!(for_ep2, vec![file.clone()]);

    // Delete removes the row and cascades the links.
    media_files.delete(file.id).await.expect("delete");
    assert!(media_files.get(file.id).await.expect("get").is_none());
    assert!(media_files
        .list_for_content(ep1.id)
        .await
        .expect("list after delete")
        .is_empty());
}

#[tokio::test]
async fn content_upsert_and_children_walk_the_tree() {
    let (_dir, db) = temp_db().await;
    let config = db.config();
    let content = db.content();

    let library = Library {
        id: LibraryId::new(),
        media_type: MediaType::Tv,
        name: "TV".to_string(),
        root_folders: vec!["/tv".to_string()],
        default_quality_profile: QualityProfileId::new(),
    };
    config.upsert_library(&library).await.unwrap();

    let series = ContentNode {
        id: ContentId::new(),
        library_id: library.id,
        media_type: MediaType::Tv,
        parent_id: None,
        kind: ContentKind::Series,
        coords: Coordinates::Episode {
            season: 0,
            episode: 0,
            absolute: None,
        },
        monitored: true,
        title_id: None,
    };
    content.upsert(&series).await.unwrap();

    let season = ContentNode {
        id: ContentId::new(),
        parent_id: Some(series.id),
        kind: ContentKind::Season,
        coords: Coordinates::SeasonPack { season: 1 },
        ..series.clone()
    };
    content.upsert(&season).await.unwrap();

    // upsert is idempotent: re-upserting a node with a changed field updates it.
    let mut season_renamed = season.clone();
    season_renamed.monitored = false;
    content.upsert(&season_renamed).await.unwrap();

    let kids = content.children(series.id).await.expect("children");
    assert_eq!(kids.len(), 1);
    assert_eq!(kids[0].id, season.id);
    assert_eq!(kids[0].kind, ContentKind::Season);
    assert!(!kids[0].monitored, "the update took effect");

    // A leaf has no children.
    assert!(content
        .children(season.id)
        .await
        .expect("leaf children")
        .is_empty());
}

#[tokio::test]
async fn config_aggregates_round_trip() {
    let (_dir, db) = temp_db().await;
    let config = db.config();

    let root = RootFolder {
        id: "rf-1".to_string(),
        path: "/data/movies".to_string(),
        name: Some("Movies".to_string()),
        enabled: true,
    };
    config.upsert_root_folder(&root).await.expect("upsert rf");
    assert_eq!(
        config.get_root_folder("rf-1").await.expect("get").as_ref(),
        Some(&root)
    );
    assert_eq!(config.list_root_folders().await.expect("list"), vec![root]);

    let indexer = IndexerConfig {
        id: IndexerId::new(),
        name: "Torznab Tracker".to_string(),
        kind: "torznab".to_string(),
        protocol: Protocol::Torrent,
        enabled: true,
        priority: 5,
        settings: serde_json::json!({"base_url": "https://t/api", "api_key": "REDACTED"}),
    };
    config.upsert_indexer(&indexer).await.expect("upsert idx");
    assert_eq!(
        config.get_indexer(indexer.id).await.expect("get"),
        Some(indexer.clone())
    );
    assert_eq!(config.list_indexers().await.expect("list"), vec![indexer]);

    let client = DownloadClientConfig {
        id: DownloadClientId::new(),
        name: "qBittorrent".to_string(),
        kind: "qbittorrent".to_string(),
        protocol: Protocol::Torrent,
        enabled: true,
        priority: 1,
        category: "cellarr".to_string(),
        settings: serde_json::json!({"host": "127.0.0.1", "port": 8080}),
    };
    config
        .upsert_download_client(&client)
        .await
        .expect("upsert client");
    assert_eq!(
        config.get_download_client(client.id).await.expect("get"),
        Some(client.clone())
    );
    assert_eq!(
        config.list_download_clients().await.expect("list"),
        vec![client]
    );

    let notification = NotificationConfig {
        id: "notif-1".to_string(),
        name: "Discord".to_string(),
        kind: "discord".to_string(),
        enabled: true,
        on_events: vec!["grab".to_string(), "import".to_string()],
        settings: serde_json::json!({"webhook_url": "https://discord/x"}),
    };
    config
        .upsert_notification(&notification)
        .await
        .expect("upsert notif");
    assert_eq!(
        config.get_notification("notif-1").await.expect("get"),
        Some(notification.clone())
    );
    assert_eq!(
        config.list_notifications().await.expect("list"),
        vec![notification]
    );
}

#[tokio::test]
async fn indexer_delete_and_enabled_filter() {
    let (_dir, db) = temp_db().await;
    let config = db.config();

    let enabled_lo = IndexerConfig {
        id: IndexerId::new(),
        name: "Alpha".to_string(),
        kind: "torznab".to_string(),
        protocol: Protocol::Torrent,
        enabled: true,
        priority: 5,
        settings: serde_json::json!({"baseUrl": "https://a/api"}),
    };
    let enabled_hi = IndexerConfig {
        id: IndexerId::new(),
        name: "Bravo".to_string(),
        kind: "newznab".to_string(),
        protocol: Protocol::Usenet,
        enabled: true,
        priority: 1,
        settings: serde_json::json!({"baseUrl": "https://b/api"}),
    };
    let disabled = IndexerConfig {
        id: IndexerId::new(),
        name: "Charlie".to_string(),
        kind: "torznab".to_string(),
        protocol: Protocol::Torrent,
        enabled: false,
        priority: 2,
        settings: serde_json::json!({"baseUrl": "https://c/api"}),
    };
    for ix in [&enabled_lo, &enabled_hi, &disabled] {
        config.upsert_indexer(ix).await.expect("upsert");
    }

    // Enabled-only, ascending priority (Bravo prio 1 before Alpha prio 5); the
    // disabled one is excluded.
    let enabled = config.list_enabled_indexers().await.expect("enabled");
    let names: Vec<&str> = enabled.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["Bravo", "Alpha"]);

    // Delete returns true for an existing row, false (idempotent) when re-run.
    assert!(config.delete_indexer(enabled_lo.id).await.expect("del"));
    assert!(!config.delete_indexer(enabled_lo.id).await.expect("del2"));
    assert!(config
        .get_indexer(enabled_lo.id)
        .await
        .expect("get")
        .is_none());
    // The others survive.
    assert_eq!(config.list_indexers().await.expect("list").len(), 2);
}

#[tokio::test]
async fn history_append_and_query_in_order() {
    let (_dir, db) = temp_db().await;
    let history = db.history();
    let content_id = ContentId::new();
    let run_id = PipelineRunId::new();

    let grab_id = cellarr_core::GrabId::new();
    let first = HistoryRecord {
        at: OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
        content_id,
        run_id,
        event: HistoryEvent::Grabbed {
            grab_id,
            release_type: Some(cellarr_core::ReleaseType::FullSeason),
        },
    };
    let second = HistoryRecord {
        at: OffsetDateTime::from_unix_timestamp(1_700_000_100).unwrap(),
        content_id,
        run_id,
        event: HistoryEvent::Imported { grab_id },
    };
    history.append(&first).await.expect("append first");
    history.append(&second).await.expect("append second");

    let records = history.for_content(content_id).await.expect("query");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].event, first.event);
    assert_eq!(records[1].event, second.event);
}

#[tokio::test]
async fn decision_log_append_and_query() {
    let (_dir, db) = temp_db().await;
    let log = db.decision_log();
    let run_id = PipelineRunId::new();

    let transition = Transition::new(Stage::Decide, Stage::Grab, TransitionKind::Advance).unwrap();
    let record = DecisionLogRecord {
        at: OffsetDateTime::now_utc(),
        run_id,
        transition,
        decision: None,
        note: Some("grabbing best candidate".to_string()),
    };
    log.append(&record).await.expect("append");

    let records = log.for_run(run_id).await.expect("query");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].note.as_deref(), Some("grabbing best candidate"));
    assert_eq!(records[0].transition, transition);
}

#[tokio::test]
async fn profile_and_custom_format_round_trip() {
    let (_dir, db) = temp_db().await;
    let profiles = db.profiles();

    let profile = QualityProfile {
        id: QualityProfileId::new(),
        name: "HD".to_string(),
        allowed_qualities: vec![1, 2, 3],
        upgrades_allowed: true,
        cutoff_quality: 3,
        min_custom_format_score: 0,
        upgrade_until_custom_format_score: 100,
        required_languages: vec!["en".to_string()],
    };
    profiles.upsert_profile(&profile).await.expect("upsert");
    let got = profiles
        .get_profile(profile.id)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(got, profile);

    let cf = CustomFormat {
        id: CustomFormatId::new(),
        name: "Bluray Source".to_string(),
        conditions: vec![Condition {
            kind: ConditionKind::Source {
                source: Source::Bluray,
            },
            required: false,
            negate: false,
        }],
        score: 50,
    };
    profiles.upsert_custom_format(&cf).await.expect("upsert cf");
    let all = profiles.custom_formats().await.expect("list");
    assert_eq!(all, vec![cf]);
}

#[tokio::test]
async fn fts_search_finds_indexed_titles() {
    let (_dir, db) = temp_db().await;
    let config = db.config();
    let content = db.content();

    let library = Library {
        id: LibraryId::new(),
        media_type: MediaType::Tv,
        name: "TV".to_string(),
        root_folders: vec!["/tv".to_string()],
        default_quality_profile: QualityProfileId::new(),
    };
    config.upsert_library(&library).await.unwrap();

    let breaking = ContentNode {
        id: ContentId::new(),
        library_id: library.id,
        media_type: MediaType::Tv,
        parent_id: None,
        kind: ContentKind::Episode,
        coords: Coordinates::Episode {
            season: 1,
            episode: 1,
            absolute: None,
        },
        monitored: true,
        title_id: None,
    };
    content.upsert(&breaking).await.unwrap();
    content
        .index_title(breaking.id, "Breaking Bad")
        .await
        .expect("index");

    let other = ContentNode {
        id: ContentId::new(),
        ..breaking.clone()
    };
    content.upsert(&other).await.unwrap();
    content
        .index_title(other.id, "The Wire")
        .await
        .expect("index");

    let hits = content.search("breaking").await.expect("search");
    assert_eq!(hits, vec![breaking.id]);

    let hits = content.search("wire").await.expect("search");
    assert_eq!(hits, vec![other.id]);

    let none = content.search("sopranos").await.expect("search");
    assert!(none.is_empty());
}

#[tokio::test]
async fn cache_put_get_and_expiry() {
    let (_dir, db) = temp_db().await;
    let cache = db.cache();

    cache.put("k1", "v1", None).await.expect("put");
    assert_eq!(cache.get("k1").await.expect("get"), Some("v1".to_string()));

    // Already-expired entry reads as absent.
    let past = OffsetDateTime::from_unix_timestamp(1).unwrap();
    cache.put("k2", "v2", Some(past)).await.expect("put");
    assert_eq!(cache.get("k2").await.expect("get"), None);

    // Future-expiry entry is returned.
    let future = OffsetDateTime::now_utc() + time::Duration::hours(1);
    cache.put("k3", "v3", Some(future)).await.expect("put");
    assert_eq!(cache.get("k3").await.expect("get"), Some("v3".to_string()));
}

#[tokio::test]
async fn writer_serializes_concurrent_writes() {
    let (_dir, db) = temp_db().await;
    let config = db.config();
    let profile_id = QualityProfileId::new();
    config
        .upsert_library(&Library {
            id: LibraryId::new(),
            media_type: MediaType::Movie,
            name: "M".to_string(),
            root_folders: vec![],
            default_quality_profile: profile_id,
        })
        .await
        .unwrap();
    let lib = config.list_libraries().await.unwrap()[0].id;

    // Fire many content upserts concurrently; the single writer-actor must
    // serialize them with no SQLITE_BUSY errors and no lost writes.
    let mut handles = Vec::new();
    for _ in 0..50 {
        let content = db.content();
        let node = movie_node(lib, ContentId::new());
        handles.push(tokio::spawn(async move { content.upsert(&node).await }));
    }
    for h in handles {
        h.await.expect("task").expect("upsert ok");
    }

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM content")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count, 50);
}

#[tokio::test]
async fn reopen_after_writes_preserves_data() {
    // Crash-safety analogue: write, drop the handle (closing the writer + pool),
    // reopen the same file, and assert the committed data survived.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("persist.db");
    let path_str = path.to_str().unwrap().to_string();

    let library_id = LibraryId::new();
    {
        let db = Database::open(&path_str).await.expect("open");
        db.config()
            .upsert_library(&Library {
                id: library_id,
                media_type: MediaType::Book,
                name: "Books".to_string(),
                root_folders: vec!["/books".to_string()],
                default_quality_profile: QualityProfileId::new(),
            })
            .await
            .expect("write");
        // db dropped here: pool closes, writer task ends.
    }

    let db = Database::open(&path_str).await.expect("reopen");
    let lib = db
        .config()
        .get_library(library_id)
        .await
        .expect("get")
        .expect("survived reopen");
    assert_eq!(lib.name, "Books");
}

#[tokio::test]
async fn verdict_decision_persists_in_log() {
    // A decision-log record carrying a full Decision round-trips through JSON.
    let (_dir, db) = temp_db().await;
    let log = db.decision_log();
    let run_id = PipelineRunId::new();

    let content_ref = ContentRef {
        id: ContentId::new(),
        library_id: LibraryId::new(),
        media_type: MediaType::Movie,
        coords: Coordinates::Movie,
    };
    let release = Release {
        indexer_id: IndexerId::new(),
        title: "X.2024.1080p-G".to_string(),
        download_url: "magnet:?x".to_string(),
        guid: None,
        protocol: Protocol::Usenet,
        size: None,
        seeders: None,
        indexer_flags: vec![],
    };
    let decision = cellarr_core::Decision {
        content_ref,
        release,
        verdict: Verdict::Grab {
            score: Score {
                quality_rank: 3,
                custom_format_score: 50,
            },
        },
    };
    let transition = Transition::new(Stage::Decide, Stage::Grab, TransitionKind::Advance).unwrap();
    let record = DecisionLogRecord {
        at: OffsetDateTime::now_utc(),
        run_id,
        transition,
        decision: Some(decision.clone()),
        note: None,
    };
    log.append(&record).await.expect("append");
    let got = log.for_run(run_id).await.expect("query");
    assert_eq!(got[0].decision, Some(decision));
}

#[tokio::test]
async fn blocklist_add_list_is_blocklisted_and_remove_round_trip() {
    use cellarr_core::blocklist::{BlocklistEntry, BlocklistRepository};

    let (_dir, db) = temp_db().await;

    // The blocklist FK requires a real library + content node.
    let library = Library {
        id: LibraryId::new(),
        media_type: MediaType::Movie,
        name: "Movies".to_string(),
        root_folders: vec!["/data/movies".to_string()],
        default_quality_profile: QualityProfileId::new(),
    };
    db.config()
        .upsert_library(&library)
        .await
        .expect("upsert library");
    let content_id = ContentId::new();
    db.content()
        .upsert(&movie_node(library.id, content_id))
        .await
        .expect("upsert content");

    let bad = Release {
        indexer_id: IndexerId::new(),
        title: "Bad.Movie.2024.1080p.BluRay-BAD".to_string(),
        download_url: "magnet:?xt=urn:btih:bad".to_string(),
        guid: Some("guid-bad".to_string()),
        protocol: Protocol::Torrent,
        size: Some(8_000_000_000),
        seeders: Some(0),
        indexer_flags: vec![],
    };
    let other = Release {
        indexer_id: IndexerId::new(),
        title: "Good.Movie.2024.1080p.WEB-DL-OK".to_string(),
        download_url: "magnet:?xt=urn:btih:ok".to_string(),
        guid: Some("guid-ok".to_string()),
        protocol: Protocol::Torrent,
        size: Some(8_000_000_000),
        seeders: Some(50),
        indexer_flags: vec![],
    };

    let blocklist = db.blocklist();
    // Not blocklisted before adding.
    assert!(!blocklist
        .is_blocklisted(content_id, &bad)
        .await
        .expect("query"));

    let entry = BlocklistEntry::from_release(
        content_id,
        &bad,
        "download failed",
        OffsetDateTime::now_utc(),
    );
    blocklist.add(&entry).await.expect("add");

    // is_blocklisted matches the added release by its stable key, and a different
    // release for the same content is NOT blocklisted.
    assert!(blocklist
        .is_blocklisted(content_id, &bad)
        .await
        .expect("query bad"));
    assert!(!blocklist
        .is_blocklisted(content_id, &other)
        .await
        .expect("query other"));

    // list returns the entry.
    let listed = blocklist.list().await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Bad.Movie.2024.1080p.BluRay-BAD");

    // add is idempotent on (content_id, release_key): re-adding refreshes, not dups.
    let refreshed = BlocklistEntry::from_release(
        content_id,
        &bad,
        "download stalled",
        OffsetDateTime::now_utc(),
    );
    blocklist.add(&refreshed).await.expect("re-add");
    let listed = blocklist.list().await.expect("list after re-add");
    assert_eq!(listed.len(), 1, "re-blocklisting the same release dedupes");
    assert_eq!(listed[0].reason, "download stalled", "reason refreshed");

    // remove clears it (idempotent on a second call), and it is grabbable again.
    let removed_id = &listed[0].id;
    assert!(blocklist.remove(removed_id).await.expect("remove"));
    assert!(
        !blocklist.remove(removed_id).await.expect("remove again"),
        "removing an already-removed entry returns false"
    );
    assert!(!blocklist
        .is_blocklisted(content_id, &bad)
        .await
        .expect("query after remove"));
    assert!(blocklist.list().await.expect("list empty").is_empty());
}
