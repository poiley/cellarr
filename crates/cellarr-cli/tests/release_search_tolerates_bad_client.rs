//! Regression test for the interactive-search 500: a SEARCH must never build a
//! download client, so a **misconfigured** download client can never fail it.
//!
//! The bug: `GET /api/v3/release` drove the daemon's [`LiveReleaseSearch`], which
//! resolved the live pipeline environment — and that resolve step *built the
//! enabled download client*. A search never grabs, so it never needs a client;
//! but a misconfigured client (e.g. a SABnzbd row whose settings JSON is missing
//! its base URL) made the build fail, the resolve error propagate, and the whole
//! request 500 — even though no download was ever going to be created.
//!
//! This test reproduces that exact shape offline: it seeds a content node, a
//! library + quality profile, and a **misconfigured** SABnzbd download client
//! (settings with `host`/`port` but no `base_url`, the synthetic-fixture shape),
//! then drives the real [`LiveReleaseSearch::search`] and asserts it returns
//! `Ok(...)` rather than `Err` — the difference between a 200 (possibly empty) and
//! a 500. With no enabled indexers, Discover yields nothing, so the candidate list
//! is empty — which is the correct, non-erroring result for a search.

use std::sync::Arc;

use async_trait::async_trait;

use cellarr_api::release_search::{ReleaseSearch, ReleaseSearchOutcome};
use cellarr_cli::pipeline::LiveReleaseSearch;
use cellarr_core::repo::ContentRepository;
use cellarr_core::{
    ContentId, ContentKind, ContentNode, ContentRef, Coordinates, DownloadClientConfig,
    DownloadClientId, Library, LibraryId, MediaType, Protocol, QualityProfile, QualityProfileId,
    QualityRanking,
};
use cellarr_db::Database;
use cellarr_media::{
    ContentCandidate, ContentLookup, MediaRegistry, MetadataLookup, MovieMeta, MovieModule,
    SeriesMeta,
};

// --- a media registry that resolves the seeded movie node ------------------

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

/// Seed a movie node + its library (with a root folder and quality profile), so
/// the search environment is "ready" — the only thing wrong is the client.
async fn seed_movie(db: &Database) -> ContentRef {
    let library_id = LibraryId::new();
    let profile = permissive_profile();
    db.profiles().upsert_profile(&profile).await.unwrap();
    let library = Library {
        id: library_id,
        media_type: MediaType::Movie,
        name: "Movies".into(),
        root_folders: vec!["/tmp/synthetic-library".into()],
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
        coords: Coordinates::Movie,
        monitored: true,
        title_id: None,
    };
    ContentRepository::upsert(&db.content(), &node)
        .await
        .unwrap();
    ContentRef::new(content_id, library_id, MediaType::Movie, Coordinates::Movie).unwrap()
}

/// Persist a SABnzbd download client whose settings JSON is **missing its base
/// URL** — the exact synthetic-fixture misconfiguration that used to 500 a
/// release search when the resolve step tried to build a live client adapter.
async fn seed_misconfigured_client(db: &Database) {
    let client = DownloadClientConfig {
        tags: Vec::new(),
        id: DownloadClientId::new(),
        name: "Synthetic SABnzbd".into(),
        kind: "sabnzbd".into(),
        protocol: Protocol::Usenet,
        enabled: true,
        priority: 1,
        category: "cellarr".into(),
        // host/port but no base_url: SabnzbdSettings does not deserialize, so
        // ConfiguredDownloadClient::from_config would error if a search built it.
        settings: serde_json::json!({ "host": "127.0.0.1", "port": 8085 }),
    };
    db.config().upsert_download_client(&client).await.unwrap();
}

#[tokio::test]
async fn release_search_does_not_500_on_a_misconfigured_download_client() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();

    let node = seed_movie(&db).await;
    seed_misconfigured_client(&db).await;
    let registry = Arc::new(movie_registry(&node));

    let search = LiveReleaseSearch::new(db.clone(), registry);

    // The whole point: this returns Ok, not Err. Before the fix the resolve step
    // built the misconfigured SABnzbd client and propagated its build error, which
    // the shim mapped to a 500. Now a search never builds a client, so it never
    // errors on one.
    let outcome = search
        .search(node.id)
        .await
        .expect("a search must not error because a download client is misconfigured");

    // No enabled indexers were seeded, so Discover yields nothing: the search ran
    // and found an (empty) candidate list — the correct 200/empty result, not a
    // 500. (Unavailable would also be a non-erroring result, but with a ready
    // library/profile the env resolves and we get Found.)
    match outcome {
        ReleaseSearchOutcome::Found(candidates) => {
            assert!(
                candidates.is_empty(),
                "with no indexers configured the search yields no candidates: {candidates:?}"
            );
        }
        ReleaseSearchOutcome::Unavailable(reason) => {
            panic!("env had a ready library/profile, expected Found, got Unavailable: {reason}");
        }
    }
}
