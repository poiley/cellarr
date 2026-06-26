//! The interactive (manual) release-search preview test.
//!
//! Drives the runner's read-only `preview_releases` path — the engine behind
//! `GET /api/v3/release` — for a seeded movie content node through a FAKE indexer
//! returning a mix of acceptable and junk releases, and asserts:
//!   - the returned candidates are RANKED (grabbable before rejected, best
//!     quality/score first),
//!   - an acceptable release is reported `rejected: false` with a real quality +
//!     score,
//!   - a junk release is reported `rejected: true` with a non-empty reason,
//!   - NOTHING is grabbed (no grab row, no history) — a preview never acquires.
//!
//! Everything is offline: a fake indexer + fake (never-driven) download client +
//! a tempfile SQLite DB + the real Movie media module over a mocked metadata
//! seam, real cellarr-parse + cellarr-decide.

use std::path::PathBuf;

use async_trait::async_trait;

use cellarr_core::{
    repo::{ContentRepository, GrabRepository, HistoryRepository},
    ContentId, ContentRef, Coordinates, CustomFormat, LibraryId, MediaType, Protocol,
    QualityProfile, QualityProfileId, QualityRanking, Release, SearchTerms,
};
use cellarr_db::Database;
use cellarr_decide::ProperRepackPolicy;
use cellarr_jobs::clock::LogicalClock;
use cellarr_jobs::runner::{PipelineRunner, RunnerConfig};
use cellarr_media::{
    ContentCandidate, ContentLookup, MediaRegistry, MetadataLookup, MovieMeta, MovieModule,
    SeriesMeta,
};

// ---------------------------------------------------------------------------
// Synthetic seams (offline). None hit a network.
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

/// A fake download client that PANICS if driven: the preview must never grab.
struct NeverDrivenClient;

#[derive(Debug, thiserror::Error)]
#[error("never-driven client error")]
struct NeverDrivenError;

#[async_trait]
impl cellarr_core::traits::DownloadClient for NeverDrivenClient {
    type Error = NeverDrivenError;
    fn name(&self) -> &str {
        "never-driven-client"
    }
    async fn add(&self, _grab: &cellarr_core::GrabRequest) -> Result<String, Self::Error> {
        panic!("preview_releases must NOT grab: download client was driven");
    }
    async fn status(
        &self,
        _download_id: &str,
    ) -> Result<cellarr_core::DownloadStatus, Self::Error> {
        panic!("preview_releases must NOT track: status was polled");
    }
    async fn remove(&self, _download_id: &str, _delete_data: bool) -> Result<(), Self::Error> {
        panic!("preview_releases must NOT remove a download");
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
        Ok(None::<SeriesMeta>)
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A profile that allows the good web/bluray tiers but **disallows low ranks**
/// (CAM/TS and the unrankable Unknown), so a junk CAM release is rejected with a
/// `QualityNotAllowed` reason while the 720p/1080p releases are grabbable.
fn web_and_bluray_profile() -> QualityProfile {
    let ranking = QualityRanking::default();
    // Allow only ranks at/above WEBDL-720p (rank 16 in the default ranking); this
    // excludes CAM (rank 2) so the junk release is rejected.
    let allowed: Vec<u32> = ranking
        .qualities
        .iter()
        .map(|q| q.rank)
        .filter(|r| *r >= 16)
        .collect();
    QualityProfile {
        id: QualityProfileId::new(),
        name: "web-and-bluray".into(),
        allowed_qualities: allowed,
        upgrades_allowed: true,
        cutoff_quality: 21,
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

/// A movie release attributed to a specific indexer id (for the priority
/// tie-break test).
fn movie_release_from(title: &str, guid: &str, indexer_id: cellarr_core::IndexerId) -> Release {
    Release {
        indexer_id,
        ..movie_release(title, guid)
    }
}

async fn seed_movie_node(db: &Database) -> ContentRef {
    let library_id = LibraryId::new();
    let library = cellarr_core::Library {
        id: library_id,
        media_type: MediaType::Movie,
        name: "movie lib".into(),
        root_folders: vec!["/tmp/synthetic".into()],
        default_quality_profile: QualityProfileId::new(),
    };
    db.config().upsert_library(&library).await.unwrap();

    let content_id = ContentId::new();
    let node = cellarr_core::ContentNode {
        tags: Vec::new(),
        id: content_id,
        library_id,
        media_type: MediaType::Movie,
        parent_id: None,
        kind: cellarr_core::ContentKind::Movie,
        series_type: cellarr_core::SeriesType::Standard,
        coords: Coordinates::Movie,
        monitored: true,
        title_id: None,
    };
    db.content().upsert(&node).await.unwrap();
    ContentRef::new(content_id, library_id, MediaType::Movie, Coordinates::Movie).unwrap()
}

fn registry_for(node: &ContentRef, title: &str) -> MediaRegistry {
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

fn runner_config(profile: QualityProfile) -> RunnerConfig {
    RunnerConfig {
        content_tag_ids: Vec::new(),
        profile,
        custom_formats: Vec::<CustomFormat>::new(),
        ranking: QualityRanking::default(),
        proper_repack_policy: ProperRepackPolicy::default(),
        library_root: PathBuf::from("/tmp/synthetic"),
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
        release_profiles: Vec::new(),
        content_tags: Vec::new(),
        permissions: Default::default(),
        extra_files: Default::default(),
        indexer_criteria: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn preview_returns_ranked_candidates_and_never_grabs() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("cellarr.sqlite");
    let db = Database::open(db_path.to_str().unwrap()).await.unwrap();

    let node = seed_movie_node(&db).await;
    let registry = registry_for(&node, "The Matrix");

    // A high-quality acceptable release, a lower-quality acceptable release, and a
    // junk title the parser cannot bucket into an allowed quality (rejected).
    let indexer = FakeIndexer {
        releases: vec![
            movie_release("The.Matrix.1999.1080p.BluRay.x264-GROUP", "guid-1080p"),
            movie_release("The.Matrix.1999.720p.WEB-DL.x264-GROUP", "guid-720p"),
            movie_release("The.Matrix.1999.CAM.junk-NOPE", "guid-junk"),
        ],
    };
    let client = NeverDrivenClient;
    let clock = LogicalClock::new(0);
    let config = runner_config(web_and_bluray_profile());

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
    let candidates = runner.preview_releases(&node).await.unwrap();

    assert!(
        candidates.len() >= 2,
        "expected at least the two acceptable candidates, got {candidates:?}"
    );

    // Ranking: every non-rejected candidate must come before every rejected one.
    let first_rejected = candidates.iter().position(|c| c.rejected);
    if let Some(idx) = first_rejected {
        assert!(
            candidates[idx..].iter().all(|c| c.rejected),
            "rejected candidates must be grouped after grabbable ones: {candidates:?}"
        );
    }

    // The best (first) candidate is grabbable, the 1080p release, with a real
    // quality and a grab/score rationale.
    let best = &candidates[0];
    assert!(!best.rejected, "the top candidate must be grabbable");
    assert!(
        best.release.title.contains("1080p"),
        "the top-ranked candidate should be the 1080p release, got {:?}",
        best.release.title
    );
    assert!(
        best.quality.rank > 0,
        "a grabbable candidate has a real quality rank"
    );
    assert!(
        !best.reason.is_empty(),
        "the reason field is always populated"
    );

    // Among the grabbable candidates, the 1080p outranks the 720p (higher rank).
    let g1080 = candidates
        .iter()
        .find(|c| c.release.title.contains("1080p"))
        .unwrap();
    let g720 = candidates
        .iter()
        .find(|c| c.release.title.contains("720p"))
        .unwrap();
    assert!(
        g1080.quality.rank > g720.quality.rank,
        "1080p should rank above 720p"
    );

    // At least one rejected candidate with a non-empty reason (the junk title).
    assert!(
        candidates
            .iter()
            .any(|c| c.rejected && !c.reason.is_empty()),
        "expected a rejected candidate carrying a reason: {candidates:?}"
    );

    // Critically: the preview grabbed NOTHING. No grab row was created, and no
    // history was appended for the node (the NeverDrivenClient would have panicked
    // if the runner had tried to grab/track).
    let history = HistoryRepository::for_content(&db.history(), node.id)
        .await
        .unwrap();
    assert!(
        history.is_empty(),
        "preview must not append history (it does not grab): {history:?}"
    );
    // And no grab exists for the candidate guids (there is no grab listing API on
    // the repo; the absence of history + the never-driven client already prove no
    // grab happened, but we also assert the grab repo has nothing for this run by
    // confirming a fresh get on a random id is None — a smoke check the table is
    // not implicitly populated).
    let missing = GrabRepository::get(&db.grabs(), cellarr_core::GrabId::new())
        .await
        .unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn indexer_priority_breaks_ties_between_equal_releases() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("cellarr.sqlite");
    let db = Database::open(db_path.to_str().unwrap()).await.unwrap();

    let node = seed_movie_node(&db).await;
    let registry = registry_for(&node, "The Matrix");

    // Two BYTE-FOR-BYTE equal-standing releases (same title => same quality + CF
    // score), differing only in which indexer returned them.
    let hi_priority = cellarr_core::IndexerId::new(); // lower number = preferred
    let lo_priority = cellarr_core::IndexerId::new();
    let indexer = FakeIndexer {
        releases: vec![
            // Deliberately list the LOWER-priority indexer's release FIRST, so a
            // pass-through (no tie-break) would rank it ahead. The priority
            // tie-break must reorder the higher-priority indexer's release to the
            // top despite the input order.
            movie_release_from(
                "The.Matrix.1999.1080p.BluRay.x264-GROUP",
                "guid-lo",
                lo_priority,
            ),
            movie_release_from(
                "The.Matrix.1999.1080p.BluRay.x264-GROUP",
                "guid-hi",
                hi_priority,
            ),
        ],
    };
    let client = NeverDrivenClient;
    let clock = LogicalClock::new(0);

    let mut config = runner_config(web_and_bluray_profile());
    // hi_priority indexer has the lower (preferred) priority number.
    config.indexer_criteria = std::collections::HashMap::from([
        (hi_priority, (cellarr_core::IndexerCriteria::default(), 1)),
        (lo_priority, (cellarr_core::IndexerCriteria::default(), 50)),
    ]);

    let runner = PipelineRunner::new(&indexer, &client, &registry, &db, &clock, &config);
    let candidates = runner.preview_releases(&node).await.unwrap();

    // Both are grabbable and equal on quality+score; the higher-priority indexer's
    // release (guid-hi) must come first purely on the indexer-priority tie-break.
    assert!(candidates.len() >= 2, "got {candidates:?}");
    assert!(!candidates[0].rejected && !candidates[1].rejected);
    assert_eq!(candidates[0].quality.rank, candidates[1].quality.rank);
    assert_eq!(
        candidates[0].release.guid.as_deref(),
        Some("guid-hi"),
        "the lower-priority-number indexer's release should win the tie, got {candidates:?}"
    );
}
