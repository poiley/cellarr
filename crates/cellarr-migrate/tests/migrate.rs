//! Integration tests for cellarr-migrate against synthetic, sanitized fixture
//! databases (see `tests/fixtures/`).
//!
//! These pin the migration contract from docs/12-migration.md:
//! - source detection (Sonarr vs Radarr by schema),
//! - mapping correctness (counts + key identities preserved),
//! - profiles/CFs reproduce equivalent decisions (cross-checked with
//!   cellarr-decide), and
//! - recognize-in-place: zero file operations for already-correct files.

use cellarr_core::repo::{ContentRepository, ProfileRepository};
use cellarr_core::{
    IndexerId, ParsedRelease, Protocol, QualityRanking, Release, Resolution, Source,
};
use cellarr_db::Database;
use cellarr_decide::MatchContext;
use cellarr_migrate::{detect_source, import, preview, SourceKind};

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// A temp-file destination cellarr DB plus its temp dir, so reads and the writer
/// actor each get their own pooled connection. (The in-memory pool pins a single
/// connection that the writer holds, which starves reads; the production layout
/// is a multi-connection file pool, so tests mirror it.)
struct DestDb {
    db: Database,
    dir: std::path::PathBuf,
}

impl DestDb {
    async fn open() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        // A per-test temp *directory* (creation is the uniqueness guard; an atomic
        // counter makes the name distinct even within the same nanosecond, which a
        // bare timestamp does not under parallel test execution).
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cellarr-migrate-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cellarr.sqlite");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        Self { db, dir }
    }
}

impl Drop for DestDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn radarr_path() -> String {
    fixture("radarr.sqlite")
}

fn sonarr_path() -> String {
    fixture("sonarr.sqlite")
}

#[tokio::test]
async fn detects_radarr_and_sonarr_by_schema() {
    assert_eq!(
        detect_source(&radarr_path()).await.unwrap(),
        SourceKind::Radarr
    );
    assert_eq!(
        detect_source(&sonarr_path()).await.unwrap(),
        SourceKind::Sonarr
    );
}

#[tokio::test]
async fn unrecognized_database_is_an_error() {
    // A fresh, empty cellarr DB on disk is not an *arr schema.
    let tmp = std::env::temp_dir().join(format!(
        "cellarr-migrate-empty-{}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&tmp);
    // Create an empty SQLite file with an unrelated table.
    {
        let db = Database::open(tmp.to_str().unwrap()).await.unwrap();
        drop(db);
    }
    let err = detect_source(tmp.to_str().unwrap()).await;
    assert!(err.is_err(), "an empty/cellarr DB must not detect as *arr");
    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn radarr_preview_counts_match_fixture() {
    let p = preview(&[&radarr_path()]).await.unwrap();
    assert_eq!(p.sources, vec![SourceKind::Radarr]);
    assert_eq!(p.library_count, 1);
    // Two movie content nodes.
    assert_eq!(p.content_count, 2);
    assert_eq!(p.item_count, 2);
    // One file recognized in place (the second movie is missing).
    assert_eq!(p.file_count, 1);
    assert_eq!(p.profile_count, 1);
    assert_eq!(p.custom_format_count, 1);
    assert_eq!(p.indexer_count, 1);
    assert_eq!(p.download_client_count, 1);
    assert_eq!(p.root_folder_count, 1);
    // Recognize in place: NEVER any file operations.
    assert_eq!(p.scheduled_file_operations, 0);
}

#[tokio::test]
async fn sonarr_preview_counts_match_fixture() {
    let p = preview(&[&sonarr_path()]).await.unwrap();
    assert_eq!(p.sources, vec![SourceKind::Sonarr]);
    assert_eq!(p.library_count, 1);
    // 1 series + 2 seasons + 4 episodes = 7 content nodes.
    assert_eq!(p.content_count, 7);
    // Four grabbable episode leaves.
    assert_eq!(p.item_count, 4);
    // Two distinct files (one shared across two episodes).
    assert_eq!(p.file_count, 2);
    assert_eq!(p.scheduled_file_operations, 0);
}

#[tokio::test]
async fn unified_import_yields_movies_and_tv_side_by_side() {
    let p = preview(&[&radarr_path(), &sonarr_path()]).await.unwrap();
    assert_eq!(p.sources, vec![SourceKind::Radarr, SourceKind::Sonarr]);
    assert_eq!(p.library_count, 2);
    // 2 movie nodes + 7 TV nodes.
    assert_eq!(p.content_count, 9);
    // 6 grabbable leaves (2 movies + 4 episodes).
    assert_eq!(p.item_count, 6);
    // Two media types present.
    assert_eq!(p.media_types.len(), 2);
    assert_eq!(p.scheduled_file_operations, 0);
}

#[tokio::test]
async fn import_writes_structure_files_and_identities() {
    let dest = DestDb::open().await;
    let db = &dest.db;
    let report = import(&[&radarr_path(), &sonarr_path()], db).await.unwrap();

    // Identities preserved: 2 movies + 1 series + 4 episodes = 7 rows with
    // external/title metadata (seasons carry no typed identity row).
    assert_eq!(report.identities_written, 2 + 1 + 4);

    // Two libraries (movies + TV).
    let libs = db.config().list_libraries().await.unwrap();
    assert_eq!(libs.len(), 2);

    // The recognized movie file is linked and queryable, in place.
    // Find the movie content node by its preserved tmdb identity.
    let movie_title_id: Option<String> =
        sqlx::query_scalar("SELECT title_id FROM movie_meta WHERE tmdb_id = 100001")
            .fetch_optional(db.pool())
            .await
            .unwrap();
    assert!(
        movie_title_id.is_some(),
        "movie identity preserved by tmdb_id"
    );

    // The shared multi-episode file links to exactly two episode nodes.
    let shared_links: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM content_file cf
         JOIN media_file m ON m.id = cf.media_file_id
         WHERE m.path LIKE '%S01E02-E03%'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(shared_links, 2, "multi-episode file satisfies two episodes");

    // The file path is recognized in place (unchanged from the source).
    let in_place: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM media_file
         WHERE path = '/movies/Synthetic Movie One (1999)/Synthetic Movie One (1999) Bluray-1080p.mkv'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(in_place, 1, "movie file recognized at its existing path");
}

#[tokio::test]
async fn imported_external_ids_are_preserved() {
    let dest = DestDb::open().await;
    let db = &dest.db;
    import(&[&sonarr_path()], db).await.unwrap();

    let (tvdb, tmdb, imdb): (Option<i64>, Option<i64>, Option<String>) =
        sqlx::query_as("SELECT tvdb_id, tmdb_id, imdb_id FROM series_meta LIMIT 1")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(tvdb, Some(900001));
    assert_eq!(tmdb, Some(800001));
    assert_eq!(imdb.as_deref(), Some("tt2000001"));
}

#[tokio::test]
async fn imported_monitored_movie_with_no_file_is_missing() {
    let dest = DestDb::open().await;
    let db = &dest.db;
    import(&[&radarr_path()], db).await.unwrap();

    // The second movie has no file, so it is a monitored-missing acquisition
    // target; the first (with a recognized file) is not.
    let missing = db.content().monitored_missing().await.unwrap();
    assert_eq!(missing.len(), 1, "exactly the file-less movie is missing");
}

#[tokio::test]
async fn imported_custom_format_reproduces_equivalent_score() {
    // Read the imported custom formats back and confirm cellarr-decide scores an
    // HDR10 release with the +50 the source profile assigned — equivalent decision.
    let dest = DestDb::open().await;
    let db = &dest.db;
    import(&[&radarr_path()], db).await.unwrap();

    let formats = db.profiles().custom_formats().await.unwrap();
    assert_eq!(formats.len(), 1);
    assert_eq!(formats[0].name, "HDR10");
    assert_eq!(
        formats[0].score, 50,
        "score carried from profile FormatItems"
    );

    let ctx = MatchContext::new(&formats).unwrap();

    // An HDR10 release matches -> +50.
    let hdr_release = release("Synthetic.Movie.One.1999.2160p.BluRay.HDR10.x265-GRP");
    let mut hdr_parsed = ParsedRelease::new(&hdr_release.title);
    hdr_parsed.source = Some(Source::Bluray);
    hdr_parsed.resolution = Some(Resolution::R2160p);
    let hdr_score = cellarr_decide::score(&hdr_release, &hdr_parsed, &formats, &ctx);
    assert_eq!(hdr_score, 50, "HDR10 release earns the imported CF score");

    // A non-HDR release does not match -> 0.
    let sdr_release = release("Synthetic.Movie.One.1999.1080p.BluRay.x264-GRP");
    let mut sdr_parsed = ParsedRelease::new(&sdr_release.title);
    sdr_parsed.source = Some(Source::Bluray);
    sdr_parsed.resolution = Some(Resolution::R1080p);
    let sdr_score = cellarr_decide::score(&sdr_release, &sdr_parsed, &formats, &ctx);
    assert_eq!(sdr_score, 0, "non-HDR release earns nothing");
}

#[tokio::test]
async fn imported_profile_reproduces_quality_gating() {
    use cellarr_decide::{decide, DecisionContext, ProperRepackPolicy};

    let dest = DestDb::open().await;
    let db = &dest.db;
    import(&[&radarr_path()], db).await.unwrap();

    // Fetch the one imported profile.
    let libs = db.config().list_libraries().await.unwrap();
    let profile_id = libs[0].default_quality_profile;
    let profile = db
        .profiles()
        .get_profile(profile_id)
        .await
        .unwrap()
        .expect("default profile imported");

    // The source allowed WEBDL-1080p and Bluray-1080p only.
    let ranking = QualityRanking::default();
    let allowed = ranking.by_name("Bluray-1080p").unwrap();
    let disallowed = ranking.by_name("Bluray-2160p").unwrap();
    assert!(profile.allowed_qualities.contains(&allowed.rank));
    assert!(!profile.allowed_qualities.contains(&disallowed.rank));

    let formats = db.profiles().custom_formats().await.unwrap();
    let ctx = DecisionContext {
        profile: &profile,
        custom_formats: &formats,
        ranking: &ranking,
        blocklisted: false,
        proper_repack_policy: ProperRepackPolicy::default(),
        indexer_criteria: Default::default(),
        indexer_priority: 0,
        content_runtime: None,
        release_profiles: &[],
        content_tags: &[],
    };

    // A node to decide for.
    let content_ref = some_movie_ref(db).await;

    // Allowed quality, nothing on disk -> grab.
    let allowed_rel = release("Synthetic.Movie.Two.2010.1080p.BluRay.x264-GRP");
    let mut allowed_parsed = ParsedRelease::new(&allowed_rel.title);
    allowed_parsed.source = Some(Source::Bluray);
    allowed_parsed.resolution = Some(Resolution::R1080p);
    let d = decide(
        content_ref.clone(),
        &allowed_rel,
        &allowed_parsed,
        None,
        &ctx,
    )
    .unwrap();
    assert!(
        matches!(d.verdict, cellarr_core::Verdict::Grab { .. }),
        "allowed quality grabs: {:?}",
        d.verdict
    );

    // Disallowed quality (2160p) -> reject as quality-not-allowed.
    let bad_rel = release("Synthetic.Movie.Two.2010.2160p.BluRay.x265-GRP");
    let mut bad_parsed = ParsedRelease::new(&bad_rel.title);
    bad_parsed.source = Some(Source::Bluray);
    bad_parsed.resolution = Some(Resolution::R2160p);
    let d2 = decide(content_ref, &bad_rel, &bad_parsed, None, &ctx).unwrap();
    assert!(
        matches!(
            d2.verdict,
            cellarr_core::Verdict::Reject {
                reason: cellarr_core::RejectReason::QualityNotAllowed
            }
        ),
        "disallowed quality rejects: {:?}",
        d2.verdict
    );
}

/// A throwaway content ref pointing at an imported movie node, for decision tests.
async fn some_movie_ref(db: &Database) -> cellarr_core::ContentRef {
    let id: String = sqlx::query_scalar("SELECT id FROM content WHERE kind = 'movie' LIMIT 1")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let content_id = cellarr_core::ContentId::from_uuid(uuid_parse(&id));
    db.content().get(content_id).await.unwrap().unwrap()
}

fn uuid_parse(s: &str) -> uuid::Uuid {
    s.parse().unwrap()
}

fn release(title: &str) -> Release {
    Release {
        indexer_id: IndexerId::new(),
        title: title.to_string(),
        download_url: "http://synthetic.invalid/dl".to_string(),
        guid: None,
        protocol: Protocol::Torrent,
        size: Some(1_000_000_000),
        seeders: Some(10),
        indexer_flags: Vec::new(),
    }
}
