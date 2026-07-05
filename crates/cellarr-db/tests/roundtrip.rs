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
    CustomFormat, CustomFormatId, DelayProfile, DelayProfileId, DownloadClientConfig,
    DownloadClientId, Grab, GrabRequest, GrabStatus, IndexerConfig, IndexerId, Library, LibraryId,
    MediaFile, MediaFileId, MediaType, NotificationConfig, PipelineRunId, PreferredProtocol,
    PreferredTerm, Protocol, QualityDefinition, QualityProfile, QualityProfileId, Release,
    ReleaseProfile, ReleaseProfileId, RootFolder, Source,
};
use cellarr_db::Database;
use time::OffsetDateTime;

mod common;

/// Open a fresh migrated database for a test on the compiled backend. The unit
/// return keeps the call sites (`let (_dir, db) = temp_db().await;`) unchanged
/// while the per-backend isolation lives in [`common::test_database`]: a private
/// SQLite temp file by default, a private Postgres schema under `--features
/// postgres`.
async fn temp_db() -> ((), Database) {
    ((), common::test_database().await)
}

fn movie_node(library_id: LibraryId, id: ContentId) -> ContentNode {
    ContentNode {
        tags: Vec::new(),
        id,
        library_id,
        media_type: MediaType::Movie,
        parent_id: None,
        kind: ContentKind::Movie,
        series_type: cellarr_core::SeriesType::Standard,
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
    for table in [
        "movie_meta",
        "series_meta",
        "episode_meta",
        "cache",
        "media_management",
    ] {
        let q = format!("SELECT COUNT(*) FROM {table}");
        let n: i64 = sqlx::query_scalar(&q)
            .fetch_one(db.pool())
            .await
            .unwrap_or_else(|e| panic!("table {table} should exist: {e}"));
        assert_eq!(n, 0);
    }
}

#[tokio::test]
async fn media_management_defaults_then_round_trips() {
    use cellarr_core::{ExtraFileImport, ImportPermissions, MediaManagement};

    let (_dir, db) = temp_db().await;
    let config = db.config();

    // With no row written, the settings resolve to defaults (the daemon's prior
    // built-in behavior) rather than erroring.
    let defaults = config.get_media_management().await.expect("get default");
    assert_eq!(defaults, MediaManagement::default());

    // Persist a customized policy and read it back unchanged.
    let mut settings = MediaManagement::default();
    settings.naming.movie_file_format = "{Movie Title}/movie.{Extension}".into();
    settings.naming.season_folder_format = "S{Season:00}".into();
    settings.permissions = ImportPermissions {
        chmod_file: Some("640".into()),
        chmod_folder: Some("750".into()),
        chown: Some("media:media".into()),
    };
    settings.extra_files = ExtraFileImport {
        enabled: true,
        extensions: vec!["srt".into(), "ass".into()],
    };
    config
        .set_media_management(&settings)
        .await
        .expect("set settings");

    let back = config.get_media_management().await.expect("get settings");
    assert_eq!(back, settings);

    // A second write replaces the single document in place (no duplicate rows).
    let mut settings2 = settings.clone();
    settings2.naming.movie_file_format = "{Movie Title}.{Extension}".into();
    config
        .set_media_management(&settings2)
        .await
        .expect("overwrite settings");
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media_management")
        .fetch_one(db.pool())
        .await
        .expect("count");
    assert_eq!(n, 1, "media_management is a singleton row");
    let back2 = config.get_media_management().await.expect("get settings 2");
    assert_eq!(back2.naming.movie_file_format, "{Movie Title}.{Extension}");
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
async fn content_metadata_round_trips_and_upserts() {
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
    let node = movie_node(library.id, ContentId::new());
    content.upsert(&node).await.unwrap();

    // A node with no metadata row reads back None.
    assert_eq!(content.metadata(node.id).await.unwrap(), None);

    // Persist the resolved facts and read them back exactly.
    let meta = cellarr_core::ContentMetadata {
        title: Some("Blade Runner".to_string()),
        year: Some(1982),
        overview: Some("A blade runner must pursue replicants.".to_string()),
        runtime: Some(117),
        air_date: Some("1982-06-25".to_string()),
        digital_date: Some("2007-12-18".to_string()),
        genres: vec!["Science Fiction".to_string(), "Thriller".to_string()],
        rating: Some(8.5),
        rating_votes: Some(14231),
    };
    content.set_metadata(node.id, &meta).await.unwrap();
    assert_eq!(content.metadata(node.id).await.unwrap(), Some(meta));

    // A re-identify overwrites the prior row (upsert), never duplicates it.
    let revised = cellarr_core::ContentMetadata {
        title: Some("Blade Runner: Final Cut".to_string()),
        year: Some(1982),
        overview: None,
        runtime: Some(118),
        air_date: Some("1982-06-25".to_string()),
        digital_date: None,
        genres: vec!["Science Fiction".to_string()],
        rating: None,
        rating_votes: None,
    };
    content.set_metadata(node.id, &revised).await.unwrap();
    assert_eq!(content.metadata(node.id).await.unwrap(), Some(revised));
    // Inline the (UUID, injection-safe) id so this raw assertion query is portable
    // across both backends — the tests don't have the crate-internal `?N`→`$N`
    // translator, and a bare `?1` is invalid on Postgres.
    let rows: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM content_meta WHERE content_id = '{}'",
        node.id
    ))
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(rows, 1, "set_metadata must upsert, not duplicate");
}

#[tokio::test]
async fn external_id_reverse_lookup_finds_the_node() {
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
    let node = movie_node(library.id, ContentId::new());
    content.upsert(&node).await.unwrap();

    // No identity linked yet → no reverse hit.
    assert_eq!(
        content
            .content_id_for_external_id(MediaType::Movie, "tmdb", "603")
            .await
            .unwrap(),
        None
    );

    // Link the tmdb id, then the reverse lookup resolves back to the node — the
    // idempotency key the add path dedups on so a re-add returns this node.
    content
        .link_external_id(node.id, MediaType::Movie, "tmdb", "603", "The Matrix")
        .await
        .unwrap();
    assert_eq!(
        content
            .content_id_for_external_id(MediaType::Movie, "tmdb", "603")
            .await
            .unwrap(),
        Some(node.id)
    );
    // A different id, a non-numeric value, and the wrong media type all miss.
    for (mt, scheme, value) in [
        (MediaType::Movie, "tmdb", "604"),
        (MediaType::Movie, "tmdb", "notanumber"),
        (MediaType::Tv, "tvdb", "603"),
    ] {
        assert_eq!(
            content
                .content_id_for_external_id(mt, scheme, value)
                .await
                .unwrap(),
            None,
            "{scheme}:{value} for {mt:?} must not match"
        );
    }
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
        tags: Vec::new(),
        id: ContentId::new(),
        library_id: library.id,
        media_type: MediaType::Tv,
        parent_id: None,
        kind: ContentKind::Series,
        series_type: cellarr_core::SeriesType::Standard,
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
        series_type: cellarr_core::SeriesType::Standard,
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
        tags: Vec::new(),
        id: IndexerId::new(),
        name: "Torznab Tracker".to_string(),
        kind: "torznab".to_string(),
        protocol: Protocol::Torrent,
        enabled: true,
        priority: 5,
        // Exercise the typed acceptance criteria through the JSON body column so a
        // round-trip proves minimumSeeders/seedCriteria/requiredFlags persist.
        criteria: cellarr_core::IndexerCriteria {
            minimum_seeders: Some(3),
            seed_ratio: Some(1.5),
            seed_time_minutes: Some(1440),
            required_flags: vec!["freeleech".to_string()],
        },
        settings: serde_json::json!({"base_url": "https://t/api", "api_key": "REDACTED"}),
    };
    config.upsert_indexer(&indexer).await.expect("upsert idx");
    assert_eq!(
        config.get_indexer(indexer.id).await.expect("get"),
        Some(indexer.clone())
    );
    assert_eq!(config.list_indexers().await.expect("list"), vec![indexer]);

    let client = DownloadClientConfig {
        tags: Vec::new(),
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
        tags: Vec::new(),
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
async fn notification_delete_and_enabled_by_kind_filter() {
    let (_dir, db) = temp_db().await;
    let config = db.config();

    let discord = NotificationConfig {
        tags: Vec::new(),
        id: "n-disc".into(),
        name: "Disc".into(),
        kind: "discord".into(),
        enabled: true,
        on_events: vec![],
        settings: serde_json::json!({ "url": "https://d/x" }),
    };
    let telegram = NotificationConfig {
        tags: Vec::new(),
        id: "n-tele".into(),
        name: "Tele".into(),
        kind: "telegram".into(),
        enabled: true,
        on_events: vec![],
        settings: serde_json::json!({ "botToken": "t", "chatId": "1" }),
    };
    let disabled_discord = NotificationConfig {
        tags: Vec::new(),
        id: "n-off".into(),
        name: "Off".into(),
        kind: "discord".into(),
        enabled: false,
        on_events: vec![],
        settings: serde_json::json!({ "url": "https://d/off" }),
    };
    for n in [&discord, &telegram, &disabled_discord] {
        config.upsert_notification(n).await.expect("upsert");
    }

    // Enabled-by-kind returns only the enabled Discord (not the disabled one,
    // not the Telegram).
    let discs = config
        .list_enabled_notifications_by_kind("discord")
        .await
        .expect("by kind");
    assert_eq!(discs, vec![discord.clone()]);
    let teles = config
        .list_enabled_notifications_by_kind("telegram")
        .await
        .expect("by kind");
    assert_eq!(teles, vec![telegram]);

    // Delete is real + idempotent.
    assert!(config.delete_notification("n-disc").await.expect("delete"));
    assert!(!config
        .delete_notification("n-disc")
        .await
        .expect("delete again"));
    let after = config
        .list_enabled_notifications_by_kind("discord")
        .await
        .expect("by kind after");
    assert!(after.is_empty());
}

#[tokio::test]
async fn indexer_delete_and_enabled_filter() {
    let (_dir, db) = temp_db().await;
    let config = db.config();

    let enabled_lo = IndexerConfig {
        tags: Vec::new(),
        id: IndexerId::new(),
        name: "Alpha".to_string(),
        kind: "torznab".to_string(),
        protocol: Protocol::Torrent,
        enabled: true,
        priority: 5,
        criteria: cellarr_core::IndexerCriteria::default(),
        settings: serde_json::json!({"baseUrl": "https://a/api"}),
    };
    let enabled_hi = IndexerConfig {
        tags: Vec::new(),
        id: IndexerId::new(),
        name: "Bravo".to_string(),
        kind: "newznab".to_string(),
        protocol: Protocol::Usenet,
        enabled: true,
        priority: 1,
        criteria: cellarr_core::IndexerCriteria::default(),
        settings: serde_json::json!({"baseUrl": "https://b/api"}),
    };
    let disabled = IndexerConfig {
        tags: Vec::new(),
        id: IndexerId::new(),
        name: "Charlie".to_string(),
        kind: "torznab".to_string(),
        protocol: Protocol::Torrent,
        enabled: false,
        priority: 2,
        criteria: cellarr_core::IndexerCriteria::default(),
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
async fn profile_list_update_delete_round_trip() {
    let (_dir, db) = temp_db().await;
    let profiles = db.profiles();

    // A fresh DB has no profiles.
    assert!(profiles.list_profiles().await.expect("list").is_empty());

    let a = QualityProfile {
        id: QualityProfileId::new(),
        name: "Bravo".to_string(),
        allowed_qualities: vec![20, 21],
        upgrades_allowed: true,
        cutoff_quality: 21,
        min_custom_format_score: 0,
        upgrade_until_custom_format_score: 100,
        required_languages: vec![],
    };
    let b = QualityProfile {
        id: QualityProfileId::new(),
        name: "Alpha".to_string(),
        allowed_qualities: vec![25],
        upgrades_allowed: false,
        cutoff_quality: 25,
        min_custom_format_score: 10,
        upgrade_until_custom_format_score: 200,
        required_languages: vec!["en".to_string()],
    };
    profiles.upsert_profile(&a).await.expect("upsert a");
    profiles.upsert_profile(&b).await.expect("upsert b");

    // list_profiles returns both, ordered by name (Alpha before Bravo).
    let listed = profiles.list_profiles().await.expect("list");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0], b);
    assert_eq!(listed[1], a);

    // upsert with the same id updates in place.
    let a_updated = QualityProfile {
        name: "Bravo Updated".to_string(),
        allowed_qualities: vec![21],
        ..a.clone()
    };
    profiles.upsert_profile(&a_updated).await.expect("update a");
    let got = profiles
        .get_profile(a.id)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(got, a_updated);
    assert_eq!(profiles.list_profiles().await.expect("list").len(), 2);

    // delete is idempotent and removes the row.
    assert!(profiles.delete_profile(a.id).await.expect("delete"));
    assert!(!profiles.delete_profile(a.id).await.expect("delete again"));
    assert!(profiles.get_profile(a.id).await.expect("get").is_none());
    let remaining = profiles.list_profiles().await.expect("list");
    assert_eq!(remaining, vec![b]);
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
        tags: Vec::new(),
        id: ContentId::new(),
        library_id: library.id,
        media_type: MediaType::Tv,
        parent_id: None,
        kind: ContentKind::Episode,
        series_type: cellarr_core::SeriesType::Standard,
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

// SQLite-specific: exercises reopening the *same on-disk file*, which is the
// single-file engine's durability story. Postgres has no file to reopen (the
// server owns durability), and `Database::open` is not compiled on that backend.
#[cfg(not(feature = "postgres"))]
#[tokio::test]
async fn reopen_after_writes_preserves_data() {
    // Crash-safety analogue: write, drop the handle (closing the writer + pool),
    // reopen the same file, and assert the committed data survived.
    let dir = tempfile::TempDir::new().unwrap();
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

#[tokio::test]
async fn delete_movie_removes_node_files_and_links() {
    let (_dir, db) = temp_db().await;
    let config = db.config();
    let content = db.content();
    let media_files = db.media_files();

    let library = Library {
        id: LibraryId::new(),
        media_type: MediaType::Movie,
        name: "Movies".to_string(),
        root_folders: vec!["/data/movies".to_string()],
        default_quality_profile: QualityProfileId::new(),
    };
    config.upsert_library(&library).await.unwrap();

    let movie = movie_node(library.id, ContentId::new());
    content.upsert(&movie).await.unwrap();
    content.index_title(movie.id, "Blade Runner").await.unwrap();
    content
        .set_metadata(
            movie.id,
            &cellarr_core::ContentMetadata {
                title: Some("Blade Runner".into()),
                year: Some(1982),
                overview: None,
                runtime: Some(117),
                air_date: None,
                digital_date: None,
                genres: Vec::new(),
                rating: None,
                rating_votes: None,
            },
        )
        .await
        .unwrap();

    let file = MediaFile {
        id: MediaFileId::new(),
        path: "/data/movies/Blade Runner (1982)/movie.mkv".to_string(),
        size: 9_000_000_000,
        quality: Quality::new("Bluray-1080p", 14),
        languages: vec!["en".to_string()],
        media_info: None,
        custom_format_score: None,
        release_type: None,
    };
    media_files.create(&file).await.unwrap();
    media_files.link(movie.id, file.id).await.unwrap();

    // The delete returns the receipt: the node id and the file path to recycle.
    let receipt = content
        .delete_movie(movie.id)
        .await
        .expect("delete")
        .expect("a movie was deleted");
    assert_eq!(receipt.content_ids, vec![movie.id]);
    assert_eq!(
        receipt.media_file_paths,
        vec!["/data/movies/Blade Runner (1982)/movie.mkv".to_string()]
    );

    // The node, its metadata, its FTS row, and the media_file row are all gone.
    assert!(content.get(movie.id).await.unwrap().is_none());
    assert!(content.metadata(movie.id).await.unwrap().is_none());
    assert!(media_files.get(file.id).await.unwrap().is_none());
    let fts: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM content_fts WHERE content_id = '{}'",
        movie.id
    ))
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(fts, 0, "FTS index row must be removed too");
    let links: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM content_file WHERE content_id = '{}'",
        movie.id
    ))
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(links, 0, "content_file link must cascade away");
}

#[tokio::test]
async fn delete_movie_wrong_kind_or_missing_is_none() {
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

    // A series node addressed as a movie deletes nothing.
    let series = ContentNode {
        tags: Vec::new(),
        id: ContentId::new(),
        library_id: library.id,
        media_type: MediaType::Tv,
        parent_id: None,
        kind: ContentKind::Series,
        series_type: cellarr_core::SeriesType::Standard,
        coords: Coordinates::Episode {
            season: 0,
            episode: 0,
            absolute: None,
        },
        monitored: true,
        title_id: None,
    };
    content.upsert(&series).await.unwrap();
    assert!(content.delete_movie(series.id).await.unwrap().is_none());
    assert!(content.get(series.id).await.unwrap().is_some(), "untouched");

    // A missing id is None, not an error.
    assert!(content
        .delete_movie(ContentId::new())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn delete_series_removes_whole_subtree() {
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

    // series -> season -> two episodes; one file linked to an episode.
    let series = ContentNode {
        tags: Vec::new(),
        id: ContentId::new(),
        library_id: library.id,
        media_type: MediaType::Tv,
        parent_id: None,
        kind: ContentKind::Series,
        series_type: cellarr_core::SeriesType::Standard,
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
        series_type: cellarr_core::SeriesType::Standard,
        coords: Coordinates::SeasonPack { season: 1 },
        ..series.clone()
    };
    content.upsert(&season).await.unwrap();
    let ep1 = ContentNode {
        id: ContentId::new(),
        parent_id: Some(season.id),
        kind: ContentKind::Episode,
        series_type: cellarr_core::SeriesType::Standard,
        coords: Coordinates::Episode {
            season: 1,
            episode: 1,
            absolute: None,
        },
        ..series.clone()
    };
    let ep2 = ContentNode {
        id: ContentId::new(),
        parent_id: Some(season.id),
        kind: ContentKind::Episode,
        series_type: cellarr_core::SeriesType::Standard,
        coords: Coordinates::Episode {
            season: 1,
            episode: 2,
            absolute: None,
        },
        ..series.clone()
    };
    content.upsert(&ep1).await.unwrap();
    content.upsert(&ep2).await.unwrap();

    let file = MediaFile {
        id: MediaFileId::new(),
        path: "/tv/Show/S01E01.mkv".to_string(),
        size: 2_000_000_000,
        quality: Quality::new("WEBDL-1080p", 13),
        languages: vec![],
        media_info: None,
        custom_format_score: None,
        release_type: None,
    };
    media_files.create(&file).await.unwrap();
    media_files.link(ep1.id, file.id).await.unwrap();

    // A history row on the episode should be cleaned by the delete.
    db.history()
        .append(&HistoryRecord {
            at: OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            content_id: ep1.id,
            run_id: PipelineRunId::new(),
            event: HistoryEvent::Imported {
                grab_id: cellarr_core::GrabId::new(),
            },
        })
        .await
        .unwrap();

    let receipt = content
        .delete_series(series.id)
        .await
        .expect("delete")
        .expect("a series was deleted");
    // The receipt covers every node in the subtree.
    assert_eq!(receipt.content_ids.len(), 4);
    for id in [series.id, season.id, ep1.id, ep2.id] {
        assert!(receipt.content_ids.contains(&id));
    }
    assert_eq!(
        receipt.media_file_paths,
        vec!["/tv/Show/S01E01.mkv".to_string()]
    );

    // Every node, the file, and the episode's history are gone.
    for id in [series.id, season.id, ep1.id, ep2.id] {
        assert!(content.get(id).await.unwrap().is_none());
    }
    assert!(media_files.get(file.id).await.unwrap().is_none());
    assert!(db.history().for_content(ep1.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn delete_movie_leaves_other_content_intact() {
    let (_dir, db) = temp_db().await;
    let config = db.config();
    let content = db.content();
    let media_files = db.media_files();

    let library = Library {
        id: LibraryId::new(),
        media_type: MediaType::Movie,
        name: "Movies".to_string(),
        root_folders: vec!["/data".to_string()],
        default_quality_profile: QualityProfileId::new(),
    };
    config.upsert_library(&library).await.unwrap();

    let keep = movie_node(library.id, ContentId::new());
    let drop = movie_node(library.id, ContentId::new());
    content.upsert(&keep).await.unwrap();
    content.upsert(&drop).await.unwrap();

    // A file shared by neither — one per movie — proves we only orphan the right one.
    let keep_file = MediaFile {
        id: MediaFileId::new(),
        path: "/data/keep.mkv".to_string(),
        size: 1,
        quality: Quality::new("Bluray-1080p", 14),
        languages: vec![],
        media_info: None,
        custom_format_score: None,
        release_type: None,
    };
    media_files.create(&keep_file).await.unwrap();
    media_files.link(keep.id, keep_file.id).await.unwrap();

    content
        .delete_movie(drop.id)
        .await
        .unwrap()
        .expect("deleted");

    // The kept movie and its file survive untouched.
    assert!(content.get(keep.id).await.unwrap().is_some());
    assert!(media_files.get(keep_file.id).await.unwrap().is_some());
    assert!(content.get(drop.id).await.unwrap().is_none());
}

#[tokio::test]
async fn custom_format_get_and_delete_round_trip() {
    let (_dir, db) = temp_db().await;
    let profiles = db.profiles();

    let cf = CustomFormat {
        id: CustomFormatId::new(),
        name: "HEVC".to_string(),
        conditions: vec![Condition {
            kind: ConditionKind::ReleaseTitle {
                pattern: "(x265|hevc)".to_string(),
            },
            required: true,
            negate: false,
        }],
        score: 25,
    };
    profiles.upsert_custom_format(&cf).await.expect("upsert");

    // GET returns it.
    let got = profiles
        .get_custom_format(cf.id)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(got, cf);

    // DELETE removes it (and is idempotent).
    assert!(profiles.delete_custom_format(cf.id).await.expect("delete"));
    assert!(profiles
        .get_custom_format(cf.id)
        .await
        .expect("get")
        .is_none());
    assert!(
        !profiles.delete_custom_format(cf.id).await.expect("delete2"),
        "second delete is a no-op false"
    );
}

#[tokio::test]
async fn delay_profile_crud_round_trip() {
    let (_dir, db) = temp_db().await;
    let profiles = db.profiles();

    let dp = DelayProfile {
        id: DelayProfileId::new(),
        enabled: true,
        preferred_protocol: PreferredProtocol::Usenet,
        usenet_delay: 30,
        torrent_delay: 60,
        bypass_if_highest_quality: true,
        tags: vec!["anime".to_string()],
        order: 1,
    };
    profiles.upsert_delay_profile(&dp).await.expect("upsert");

    // GET + LIST return it.
    assert_eq!(
        profiles
            .get_delay_profile(dp.id)
            .await
            .expect("get")
            .expect("present"),
        dp
    );
    let list = profiles.list_delay_profiles().await.expect("list");
    assert_eq!(list, vec![dp.clone()]);

    // UPDATE (same id) replaces it; LIST is ordered by `order`.
    let mut dp2 = DelayProfile {
        id: dp.id,
        enabled: false,
        preferred_protocol: PreferredProtocol::Torrent,
        usenet_delay: 0,
        torrent_delay: 15,
        bypass_if_highest_quality: false,
        tags: Vec::new(),
        order: 0,
    };
    profiles.upsert_delay_profile(&dp2).await.expect("update");
    let after = profiles
        .get_delay_profile(dp.id)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(after, dp2);

    // Add a second profile and confirm order-ascending listing.
    dp2.order = 5;
    profiles.upsert_delay_profile(&dp2).await.expect("update2");
    let second = DelayProfile {
        id: DelayProfileId::new(),
        enabled: true,
        preferred_protocol: PreferredProtocol::Either,
        usenet_delay: 0,
        torrent_delay: 0,
        bypass_if_highest_quality: false,
        tags: Vec::new(),
        order: 2,
    };
    profiles
        .upsert_delay_profile(&second)
        .await
        .expect("upsert2");
    let ordered = profiles.list_delay_profiles().await.expect("list");
    assert_eq!(ordered.len(), 2);
    assert_eq!(ordered[0].order, 2, "lower order first");
    assert_eq!(ordered[1].order, 5);

    // DELETE removes one, idempotently.
    assert!(profiles.delete_delay_profile(dp.id).await.expect("delete"));
    assert!(!profiles.delete_delay_profile(dp.id).await.expect("delete2"));
    assert_eq!(profiles.list_delay_profiles().await.expect("list").len(), 1);
}

#[tokio::test]
async fn release_profile_crud_round_trip() {
    let (_dir, db) = temp_db().await;
    let profiles = db.profiles();

    let rp = ReleaseProfile {
        id: ReleaseProfileId::new(),
        name: "anime".to_string(),
        enabled: true,
        tags: vec![3, 7],
        required: vec!["bluray".to_string(), "/x26[45]/".to_string()],
        ignored: vec!["cam".to_string()],
        preferred: vec![
            PreferredTerm {
                term: "remux".to_string(),
                score: 100,
            },
            PreferredTerm {
                term: "/atmos/".to_string(),
                score: -25,
            },
        ],
    };
    profiles.upsert_release_profile(&rp).await.expect("upsert");

    // GET + LIST return it losslessly (terms, scores, tag ids all round-trip).
    assert_eq!(
        profiles
            .get_release_profile(rp.id)
            .await
            .expect("get")
            .expect("present"),
        rp
    );
    assert_eq!(
        profiles.list_release_profiles().await.expect("list"),
        vec![rp.clone()]
    );

    // UPDATE (same id) replaces it.
    let rp2 = ReleaseProfile {
        id: rp.id,
        name: "anime-v2".to_string(),
        enabled: false,
        tags: Vec::new(),
        required: Vec::new(),
        ignored: vec!["x265".to_string()],
        preferred: Vec::new(),
    };
    profiles.upsert_release_profile(&rp2).await.expect("update");
    assert_eq!(
        profiles
            .get_release_profile(rp.id)
            .await
            .expect("get")
            .expect("present"),
        rp2
    );

    // A second profile; list is ordered by name ascending.
    let second = ReleaseProfile::new("aardvark");
    profiles
        .upsert_release_profile(&second)
        .await
        .expect("upsert2");
    let ordered = profiles.list_release_profiles().await.expect("list");
    assert_eq!(ordered.len(), 2);
    assert_eq!(ordered[0].name, "aardvark", "name-ascending");
    assert_eq!(ordered[1].name, "anime-v2");

    // DELETE removes one, idempotently.
    assert!(profiles
        .delete_release_profile(rp.id)
        .await
        .expect("delete"));
    assert!(!profiles
        .delete_release_profile(rp.id)
        .await
        .expect("delete2"));
    assert_eq!(
        profiles.list_release_profiles().await.expect("list").len(),
        1
    );
}

#[tokio::test]
async fn quality_definition_edit_persists_and_merges_into_ranking() {
    let (_dir, db) = temp_db().await;
    let profiles = db.profiles();

    // A fresh DB has no overrides: the ranking equals the code default, and the
    // edited bucket starts with no size bounds.
    let base = profiles.quality_ranking().await.expect("ranking");
    let target = base
        .by_name("Bluray-1080p")
        .expect("Bluray-1080p present in catalogue");
    let before = base
        .definition_for_rank(target.rank)
        .expect("definition present");
    assert_eq!(before.min_size_per_min, None);
    assert_eq!(before.max_size_per_min, None);
    assert_eq!(before.title, None);

    // Persist an edit: title + size bounds + preferred, keyed by canonical name.
    let edited = QualityDefinition {
        name: "Bluray-1080p".to_string(),
        title: Some("HD Bluray".to_string()),
        rank: target.rank,
        min_size_per_min: Some(10),
        max_size_per_min: Some(200),
        preferred_size_per_min: Some(100),
    };
    profiles
        .upsert_quality_definition(&edited)
        .await
        .expect("upsert quality definition");

    // The merged ranking reflects the edit while keeping the canonical name/rank.
    let after = profiles.quality_ranking().await.expect("ranking2");
    let def = after
        .definition_for_rank(target.rank)
        .expect("definition present");
    assert_eq!(def.name, "Bluray-1080p", "canonical name unchanged");
    assert_eq!(def.rank, target.rank, "rank unchanged");
    assert_eq!(def.title.as_deref(), Some("HD Bluray"));
    assert_eq!(def.display_title(), "HD Bluray");
    assert_eq!(def.min_size_per_min, Some(10));
    assert_eq!(def.max_size_per_min, Some(200));
    assert_eq!(def.preferred_size_per_min, Some(100));

    // Every other bucket is untouched by the override.
    let unedited = after.by_name("WEBDL-1080p").expect("WEBDL-1080p present");
    let unedited_def = after.definition_for_rank(unedited.rank).expect("present");
    assert_eq!(unedited_def.min_size_per_min, None);
    assert_eq!(unedited_def.title, None);

    // A second upsert (same name) replaces the prior override, not appends.
    let re_edited = QualityDefinition {
        name: "Bluray-1080p".to_string(),
        title: None,
        rank: target.rank,
        min_size_per_min: Some(50),
        max_size_per_min: None,
        preferred_size_per_min: None,
    };
    profiles
        .upsert_quality_definition(&re_edited)
        .await
        .expect("re-upsert");
    let overrides = profiles
        .quality_definition_overrides()
        .await
        .expect("overrides");
    assert_eq!(overrides.len(), 1, "upsert replaces, never duplicates");
    let final_ranking = profiles.quality_ranking().await.expect("ranking3");
    let final_def = final_ranking
        .definition_for_rank(target.rank)
        .expect("present");
    assert_eq!(final_def.min_size_per_min, Some(50));
    assert_eq!(final_def.max_size_per_min, None);
    assert_eq!(final_def.title, None, "title cleared by the second edit");
    assert_eq!(final_def.display_title(), "Bluray-1080p");
}

#[tokio::test]
async fn pending_release_records_earliest_sighting_and_clears() {
    let (_dir, db) = temp_db().await;
    let pending = db.pending_releases();
    let content_id = ContentId::new();
    let release = Release {
        indexer_id: IndexerId::new(),
        title: "Show.S01E01.1080p.WEB-DL-GRP".to_string(),
        download_url: "magnet:?x".to_string(),
        guid: Some("guid-1".to_string()),
        protocol: Protocol::Torrent,
        size: None,
        seeders: None,
        indexer_flags: Vec::new(),
    };

    // First sighting at t=100 is recorded and returned.
    let first = pending
        .record_seen(content_id, &release, 100)
        .await
        .expect("record");
    assert_eq!(first, 100);

    // A LATER sighting (t=500) does not move the clock — the earliest stands.
    let again = pending
        .record_seen(content_id, &release, 500)
        .await
        .expect("record2");
    assert_eq!(again, 100, "re-seeing keeps the earliest first-seen");

    // It is listed for the content.
    let held = pending.list_for_content(content_id).await.expect("list");
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].first_seen_at, 100);
    assert_eq!(held[0].protocol, Protocol::Torrent);

    // Clearing removes it (idempotently).
    assert!(pending.clear(content_id, &release).await.expect("clear"));
    assert!(pending
        .list_for_content(content_id)
        .await
        .expect("list")
        .is_empty());
    assert!(!pending.clear(content_id, &release).await.expect("clear2"));
}

#[tokio::test]
async fn auth_config_round_trips_and_defaults_open() {
    use cellarr_core::{AuthConfig, AuthMethod};

    let (_dir, db) = temp_db().await;
    let auth = db.auth();

    // No row yet → the open default (no auth, no credential).
    let cfg = auth.get_config().await.expect("get default");
    assert_eq!(cfg, AuthConfig::open());
    assert_eq!(cfg.method, AuthMethod::None);
    assert!(!cfg.has_credential());

    // Persist a Forms config with a credential (a HASH, never plaintext).
    let stored = AuthConfig {
        method: AuthMethod::Forms,
        username: Some("admin".into()),
        password_hash: Some("$argon2id$v=19$m=19456,t=2,p=1$abc$def".into()),
    };
    auth.set_config(&stored).await.expect("set");
    let back = auth.get_config().await.expect("get");
    assert_eq!(back, stored);
    assert!(back.is_effectively_enforced());

    // Upsert in place (single-row): a method change overwrites, not appends.
    let changed = AuthConfig {
        method: AuthMethod::Basic,
        ..stored.clone()
    };
    auth.set_config(&changed).await.expect("set2");
    assert_eq!(
        auth.get_config().await.expect("get2").method,
        AuthMethod::Basic
    );
}

#[tokio::test]
async fn auth_sessions_lifecycle() {
    let (_dir, db) = temp_db().await;
    let auth = db.auth();

    // Create a session valid until t=1000.
    auth.create_session("tok-abc", "admin", 0, 1000)
        .await
        .expect("create");

    // Looked up before expiry → present.
    let live = auth.get_session("tok-abc", 500).await.expect("get");
    assert!(live.is_some());
    assert_eq!(live.unwrap().username, "admin");

    // After expiry → absent (no validity oracle, same as unknown).
    assert!(auth
        .get_session("tok-abc", 1500)
        .await
        .expect("get expired")
        .is_none());
    assert!(auth
        .get_session("nope", 100)
        .await
        .expect("get unknown")
        .is_none());

    // Delete (logout) is idempotent.
    assert!(auth.delete_session("tok-abc").await.expect("del"));
    assert!(!auth.delete_session("tok-abc").await.expect("del2"));
    assert!(auth
        .get_session("tok-abc", 100)
        .await
        .expect("gone")
        .is_none());

    // delete_all_sessions revokes everything; purge_expired sweeps stale rows.
    auth.create_session("s1", "admin", 0, 10).await.expect("s1");
    auth.create_session("s2", "admin", 0, 100)
        .await
        .expect("s2");
    let purged = auth.purge_expired_sessions(50).await.expect("purge");
    assert_eq!(purged, 1, "only the t<=50 expiry is swept");
    auth.delete_all_sessions().await.expect("nuke");
    assert!(auth
        .get_session("s2", 5)
        .await
        .expect("after nuke")
        .is_none());
}

#[tokio::test]
async fn tag_vocabulary_and_content_tag_association_round_trip() {
    let (_dir, db) = temp_db().await;
    let config = db.config();
    let content = db.content();
    let tags = db.tags();

    // The persisted tag vocabulary: dense ids from 1, case-insensitive de-dup.
    let anime = tags.create("Anime").await.expect("create anime");
    let uhd = tags.create("4K").await.expect("create 4k");
    assert_eq!(anime.id, 1);
    assert_eq!(uhd.id, 2);
    // A case-insensitive duplicate returns the existing tag, not a new id.
    let dup = tags.create("anime").await.expect("dup");
    assert_eq!(dup.id, anime.id);
    assert_eq!(tags.list().await.expect("list").len(), 2);

    // A library + a movie node to tag.
    let library = Library {
        id: LibraryId::new(),
        media_type: MediaType::Movie,
        name: "Movies".to_string(),
        root_folders: vec!["/data".to_string()],
        default_quality_profile: QualityProfileId::new(),
    };
    config.upsert_library(&library).await.unwrap();
    let node = movie_node(library.id, ContentId::new());
    content.upsert(&node).await.expect("upsert content");

    // Set/get content tags round-trips, ascending and de-duplicated.
    content
        .set_tags(node.id, &[uhd.id, anime.id, anime.id])
        .await
        .expect("set tags");
    assert_eq!(
        content.get_tags(node.id).await.expect("get tags"),
        vec![anime.id, uhd.id]
    );
    // get_node populates the tags onto the node.
    let fetched = content
        .get_node(node.id)
        .await
        .expect("get node")
        .expect("present");
    assert_eq!(fetched.tags, vec![anime.id, uhd.id]);

    // Labels resolve for the delay-profile (label-keyed) path.
    let mut labels = tags.labels_for(&[anime.id, uhd.id]).await.expect("labels");
    labels.sort();
    assert_eq!(labels, vec!["4K".to_string(), "Anime".to_string()]);

    // Replacing the tag set rewrites it wholesale; an empty set clears it.
    content.set_tags(node.id, &[uhd.id]).await.expect("retag");
    assert_eq!(content.get_tags(node.id).await.unwrap(), vec![uhd.id]);
    content.set_tags(node.id, &[]).await.expect("clear");
    assert!(content.get_tags(node.id).await.unwrap().is_empty());

    // Deleting a tag cascades the association away (re-tag, then delete).
    content.set_tags(node.id, &[anime.id]).await.unwrap();
    assert!(tags.delete(anime.id).await.expect("delete"));
    assert!(
        content.get_tags(node.id).await.unwrap().is_empty(),
        "deleting a tag detaches it from every node it tagged"
    );
    assert!(tags.get(anime.id).await.expect("get").is_none());
}
