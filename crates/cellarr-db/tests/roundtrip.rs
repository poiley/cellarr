//! Integration tests for cellarr-db against a tempfile SQLite database.
//!
//! These exercise the real on-disk path: open + migrate, the writer-actor for
//! every mutation, the repository trait implementations, and FTS search. A
//! per-test `tempfile::TempDir` keeps runs hermetic and isolated.

use cellarr_core::decision::{Score, Verdict};
use cellarr_core::history::{DecisionLogRecord, HistoryEvent, HistoryRecord};
use cellarr_core::pipeline::{Stage, Transition, TransitionKind};
use cellarr_core::repo::{
    ContentRepository, DecisionLogRepository, GrabRepository, HistoryRepository, ProfileRepository,
};
use cellarr_core::{
    Condition, ConditionKind, ContentId, ContentRef, Coordinates, CustomFormat, CustomFormatId,
    DownloadClientId, GrabRequest, IndexerId, Library, LibraryId, MediaType, PipelineRunId,
    Protocol, QualityProfile, QualityProfileId, Release, Source,
};
use cellarr_db::{ContentNode, Database};
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
        kind: "movie".to_string(),
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
    let media_file_id = cellarr_core::MediaFileId::new();
    db.writer()
        .submit({
            let cid = have.id.to_string();
            let mfid = media_file_id.to_string();
            move |conn| {
                Box::pin(async move {
                    sqlx::query("INSERT INTO media_file (id, path, size) VALUES (?1, ?2, 100)")
                        .bind(&mfid)
                        .bind("/data/have.mkv")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query(
                        "INSERT INTO content_file (content_id, media_file_id) VALUES (?1, ?2)",
                    )
                    .bind(&cid)
                    .bind(&mfid)
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            }
        })
        .await
        .expect("link file");

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
    };

    let id = grabs.create(&request).await.expect("create grab");
    let fetched = grabs.get(id).await.expect("get").expect("present");
    assert_eq!(fetched, request);

    assert!(grabs
        .get(cellarr_core::GrabId::new())
        .await
        .expect("get missing")
        .is_none());
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
        event: HistoryEvent::Grabbed { grab_id },
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
        kind: "series".to_string(),
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
