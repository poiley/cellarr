//! Import-list sync, end-to-end through the real db + sync orchestrator.
//!
//! HERMETIC: no live services and no credentials. The list source is a MOCK
//! ([`MockSourceFactory`]) that returns a configured [`FetchResult`] verbatim, so
//! both the confirmed-good path and the failed/empty-because-errored path are
//! driven exactly. The persistence is a real tempfile-backed SQLite `Database`.
//!
//! The central assertion is **the safeguard**: a list source that errors (or
//! returns empty due to error) removes/cleans NOTHING and leaves the library
//! intact, while a real successful list adds the monitored items it carries.

use std::sync::Arc;

use async_trait::async_trait;
use cellarr_core::importlist::{
    CleanAction, FetchResult, ImportListConfig, ImportListExclusion, ImportListItem,
    ImportListRepository,
};
use cellarr_core::repo::ContentRepository;
use cellarr_core::{
    ContentId, ContentKind, ContentNode, Coordinates, Library, LibraryId, MediaType,
    QualityProfileId,
};
use cellarr_db::Database;
use cellarr_jobs::importlists::sources::MockSourceFactory;
use cellarr_jobs::importlists::{ImportListSync, LibraryIndex};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A [`LibraryIndex`] that reports a fixed set of present external-id keys, so the
/// add-de-duplication path is exercised without depending on the (gapped) content
/// external-id index.
struct FixedIndex {
    keys: Vec<(String, String)>,
}

#[async_trait]
impl LibraryIndex for FixedIndex {
    async fn existing_keys(
        &self,
        _media_type: MediaType,
    ) -> Result<Vec<(String, String)>, cellarr_db::DbError> {
        Ok(self.keys.clone())
    }
}

async fn db_with_movie_library() -> (Database, tempfile::TempDir, LibraryId) {
    // A tempfile-backed DB (not in-memory): the writer actor holds a dedicated
    // connection, so reads need a multi-connection pool to avoid a self-deadlock —
    // `open_in_memory` pins the pool to one connection and would time out.
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(tmp.path().join("cellarr.sqlite").to_str().unwrap())
        .await
        .unwrap();
    let library_id = LibraryId::new();
    db.config()
        .upsert_library(&Library {
            id: library_id,
            media_type: MediaType::Movie,
            name: "Movies".into(),
            root_folders: vec!["/tmp/movies".into()],
            default_quality_profile: QualityProfileId::new(),
        })
        .await
        .unwrap();
    (db, tmp, library_id)
}

fn movie_list(clean: CleanAction) -> ImportListConfig {
    ImportListConfig {
        id: "list-1".into(),
        name: "Watchlist".into(),
        kind: "mock".into(),
        enabled: true,
        media_type: MediaType::Movie,
        monitored: true,
        clean_action: clean,
        quality_profile_id: None,
        last_successful_sync: None,
        settings: serde_json::Value::Null,
    }
}

fn item(id: &str, title: &str) -> ImportListItem {
    ImportListItem {
        id_type: "tmdb".into(),
        id_value: id.into(),
        title: title.into(),
        year: Some(2010),
        media_type: MediaType::Movie,
    }
}

/// Count library root content nodes (movies) in a library.
async fn movie_count(db: &Database, library: LibraryId) -> usize {
    db.content().roots(library).await.unwrap().len()
}

// ---------------------------------------------------------------------------
// A real successful list adds the items.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn confirmed_good_list_adds_monitored_items() {
    let (db, _tmp, library) = db_with_movie_library().await;
    let list = movie_list(CleanAction::None);
    db.import_lists().upsert(&list).await.unwrap();

    assert_eq!(movie_count(&db, library).await, 0);

    let factory = Arc::new(MockSourceFactory::new(FetchResult::Fetched(vec![
        item("100", "Heat"),
        item("200", "Collateral"),
    ])));
    let sync = ImportListSync::new(db.clone(), factory);

    let reports = sync.sync_all().await.unwrap();
    assert_eq!(reports.len(), 1);
    assert!(reports[0].fetch_succeeded);
    assert_eq!(reports[0].added, 2);
    assert_eq!(reports[0].cleaned, 0);

    // Two new monitored movie nodes landed in the library.
    let roots = db.content().roots(library).await.unwrap();
    assert_eq!(roots.len(), 2);
    assert!(roots.iter().all(|n| n.monitored));

    // The confirmed-good sync stamped last_successful_sync.
    let stored = db.import_lists().get("list-1").await.unwrap().unwrap();
    assert!(stored.last_successful_sync.is_some());
}

// ---------------------------------------------------------------------------
// THE SAFEGUARD: a failing fetch changes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn failed_fetch_with_clean_remove_removes_nothing() {
    let (db, _tmp, library) = db_with_movie_library().await;

    // Pre-populate the library with 3 movies (the things a buggy clean would wipe).
    for (i, title) in ["A", "B", "C"].iter().enumerate() {
        let node = ContentNode {
            id: ContentId::new(),
            library_id: library,
            media_type: MediaType::Movie,
            parent_id: None,
            kind: ContentKind::Movie,
            coords: Coordinates::Movie,
            monitored: true,
            title_id: None,
        };
        db.content().upsert(&node).await.unwrap();
        db.content()
            .index_title(node.id, &format!("{title}-{i}"))
            .await
            .unwrap();
    }
    assert_eq!(movie_count(&db, library).await, 3);

    // A list configured to REMOVE missing items — the catastrophe setup — whose
    // source ERRORS (returns Failed, i.e. empty-because-errored).
    let list = movie_list(CleanAction::Remove);
    db.import_lists().upsert(&list).await.unwrap();

    let factory = Arc::new(MockSourceFactory::new(FetchResult::Failed(
        "auth token expired".into(),
    )));
    let sync = ImportListSync::new(db.clone(), factory);

    let reports = sync.sync_all().await.unwrap();
    assert_eq!(reports.len(), 1);
    assert!(
        !reports[0].fetch_succeeded,
        "fetch should be reported failed"
    );
    assert_eq!(reports[0].added, 0);
    assert_eq!(reports[0].cleaned, 0, "a failed fetch cleans NOTHING");
    assert_eq!(
        reports[0].failure_reason.as_deref(),
        Some("auth token expired")
    );

    // The library is fully intact — nothing removed.
    assert_eq!(
        movie_count(&db, library).await,
        3,
        "the library must be untouched after a failed fetch"
    );

    // And the failed fetch did NOT stamp last_successful_sync.
    let stored = db.import_lists().get("list-1").await.unwrap().unwrap();
    assert!(
        stored.last_successful_sync.is_none(),
        "a failed fetch must never look like a recent good sync"
    );
}

// ---------------------------------------------------------------------------
// A *confirmed-good* empty list DOES gate a clean (contrast with the failure).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn confirmed_empty_list_gates_clean_but_failed_one_does_not() {
    let (db, _tmp, _library) = db_with_movie_library().await;
    let list = movie_list(CleanAction::Remove);
    db.import_lists().upsert(&list).await.unwrap();

    // The library is reported (via the index) to contain two items.
    let index = Arc::new(FixedIndex {
        keys: vec![("tmdb".into(), "1".into()), ("tmdb".into(), "2".into())],
    });

    // Confirmed-good EMPTY fetch -> both present items are clean-eligible.
    let good = ImportListSync::with_library_index(
        db.clone(),
        Arc::new(MockSourceFactory::new(FetchResult::Fetched(vec![]))),
        index.clone(),
    );
    let r = good.sync_one(&list).await.unwrap();
    assert!(r.fetch_succeeded);
    assert_eq!(r.cleaned, 2, "a confirmed-good empty list gates the clean");

    // The SAME empty symptom from a failure -> nothing clean-eligible.
    let bad = ImportListSync::with_library_index(
        db.clone(),
        Arc::new(MockSourceFactory::new(FetchResult::Failed("503".into()))),
        index,
    );
    let r = bad.sync_one(&list).await.unwrap();
    assert!(!r.fetch_succeeded);
    assert_eq!(r.cleaned, 0);
}

// ---------------------------------------------------------------------------
// Exclusions suppress an add.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn excluded_items_are_not_added() {
    let (db, _tmp, library) = db_with_movie_library().await;
    let list = movie_list(CleanAction::None);
    db.import_lists().upsert(&list).await.unwrap();

    db.import_lists()
        .upsert_exclusion(&ImportListExclusion {
            id: "ex-1".into(),
            id_type: "tmdb".into(),
            id_value: "200".into(),
            title: "Collateral".into(),
        })
        .await
        .unwrap();

    let factory = Arc::new(MockSourceFactory::new(FetchResult::Fetched(vec![
        item("100", "Heat"),
        item("200", "Collateral"),
    ])));
    let sync = ImportListSync::new(db.clone(), factory);
    let reports = sync.sync_all().await.unwrap();

    assert_eq!(reports[0].added, 1, "the excluded item is skipped");
    assert_eq!(movie_count(&db, library).await, 1);
}

// ---------------------------------------------------------------------------
// Blocked-on-key live sources fail gracefully (inert), never falsely empty.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn live_sources_without_credentials_fail_gracefully() {
    use cellarr_jobs::importlists::sources::LiveSourceFactory;
    use cellarr_jobs::importlists::SourceFactory;

    let factory = LiveSourceFactory;
    for kind in ["trakt", "tmdb", "plex"] {
        let mut cfg = movie_list(CleanAction::Remove);
        cfg.kind = kind.into();
        cfg.settings = serde_json::Value::Null; // no credentials
        let source = factory.build(&cfg).expect("kind is known");
        let result = source.fetch().await;
        assert!(
            matches!(result, FetchResult::Failed(_)),
            "{kind} without creds must be a graceful Failed, never an empty Fetched"
        );
    }
}
