//! What the engine does when the tracker misbehaves.
//!
//! Every case here is a bug that reached production, and none of them could be
//! reproduced by a fixture that answers every request the same way. They are
//! written against [`ScriptedFetcher`], so each failure mode is deterministic and
//! costs microseconds — the opposite of diagnosing them against a live tracker,
//! where "my code is wrong" and "the site is angry right now" look identical.

mod common;

use std::sync::Arc;

use cellarr_core::{Indexer, IndexerId, SearchTerms};
use cellarr_indexers::cardigann::CardigannIndexer;
use cellarr_indexers::http::Fetcher;
use cellarr_indexers::{Definition, HostRateLimiter};

use common::{Reply, ScriptedFetcher, DEFINITION, EMPTY_HTML, ROWS_HTML};

fn engine(fetcher: Arc<ScriptedFetcher>, yaml: &str) -> CardigannIndexer {
    CardigannIndexer::with_deps(
        IndexerId::new(),
        Definition::from_yaml(yaml).expect("parse definition"),
        std::collections::BTreeMap::new(),
        fetcher as Arc<dyn Fetcher>,
        Arc::new(HostRateLimiter::conservative_default()),
    )
}

fn terms(query: &str) -> SearchTerms {
    SearchTerms {
        queries: vec![query.to_string()],
        ..SearchTerms::default()
    }
}

/// A tracker that answers, then starts refusing, must not turn a partial result
/// into a hard failure — the releases it did serve are still good.
#[tokio::test]
async fn a_rate_limit_part_way_through_does_not_lose_what_was_served() {
    let fetcher =
        Arc::new(ScriptedFetcher::serving(ROWS_HTML).rate_limited_after(1, Reply::Status(429)));
    let releases = engine(Arc::clone(&fetcher), DEFINITION)
        .search(&terms("example"))
        .await
        .expect("a later 429 must not fail the whole search");

    assert_eq!(releases.len(), 2, "the first page's rows survive");
    assert!(fetcher.count() >= 1);
}

/// The shape of the ext.to outage: the site is up, but one combination of query
/// parameters times out its origin. A fixture serving one page for everything
/// cannot express this, which is why it took a live investigation to find.
#[tokio::test]
async fn a_tracker_that_fails_only_for_one_query_combination_is_visible() {
    let fetcher = Arc::new(
        ScriptedFetcher::serving(ROWS_HTML).when_url_contains("q=cursed", Reply::Status(522)),
    );
    let engine = engine(Arc::clone(&fetcher), DEFINITION);

    let ok = engine
        .search(&terms("example"))
        .await
        .expect("healthy query");
    assert_eq!(ok.len(), 2);

    let bad = engine.search(&terms("cursed")).await;
    assert!(
        bad.is_err(),
        "the failing combination must surface, not be silently empty"
    );

    // And the failure is attributable: the exact request that broke is recorded.
    assert!(
        fetcher.urls().iter().any(|u| u.contains("q=cursed")),
        "{:?}",
        fetcher.urls()
    );
}

/// A 429 on the *first* request is a failure, not an empty result set. Reporting
/// "no releases" for a refused request is how a rate-limited indexer silently
/// looks like a working one with nothing to offer.
#[tokio::test]
async fn a_refused_first_request_is_an_error_not_an_empty_result() {
    let fetcher = Arc::new(ScriptedFetcher::serving(ROWS_HTML).then([Reply::Status(429)]));
    let result = engine(fetcher, DEFINITION).search(&terms("example")).await;

    match result {
        Err(cellarr_indexers::IndexerError::Status { status, .. }) => assert_eq!(status, 429),
        other => panic!("expected a surfaced 429, got {other:?}"),
    }
}

/// A request that never completes must fail that search rather than hang it.
#[tokio::test]
async fn a_request_that_never_completes_fails_the_search() {
    let fetcher = Arc::new(ScriptedFetcher::serving(ROWS_HTML).then([Reply::Timeout]));
    assert!(engine(fetcher, DEFINITION)
        .search(&terms("example"))
        .await
        .is_err());
}

/// Cells carry a CSS-hidden label; both values parsed as `None` in production for
/// every release, silently, because neither field is required to build one.
#[tokio::test]
async fn labelled_size_and_seeder_cells_are_read() {
    let fetcher = Arc::new(ScriptedFetcher::serving(ROWS_HTML));
    let releases = engine(fetcher, DEFINITION)
        .search(&terms("example"))
        .await
        .expect("search");

    assert_eq!(releases[0].seeders, Some(42), "'Seeds 42' must parse");
    assert_eq!(
        releases[0].size,
        Some((2.1 * 1024.0 * 1024.0 * 1024.0) as u64),
        "'Size 2.1 GB' must parse"
    );
}

/// A tracker that publishes a courtesy interval is stating the rate at which it
/// stops answering. The field was parsed away entirely, so the engine out-ran it.
#[tokio::test]
async fn a_published_request_delay_paces_the_engine() {
    let yaml = DEFINITION.replace("type: public", "type: public\nrequestDelay: 1");
    let def = Definition::from_yaml(&yaml).expect("parse");
    assert_eq!(def.request_delay, Some(1.0), "requestDelay must be read");

    let fetcher = Arc::new(ScriptedFetcher::serving(ROWS_HTML));
    let started = std::time::Instant::now();
    let engine = engine(Arc::clone(&fetcher), &yaml);
    engine.search(&terms("one")).await.expect("search");
    engine.search(&terms("two")).await.expect("search");

    assert!(
        started.elapsed() >= std::time::Duration::from_secs(1),
        "two requests must be spaced by the published delay, took {:?}",
        started.elapsed()
    );
}

/// An empty page is a legitimate answer, not a fault — it must not be mistaken
/// for one, or a genuinely empty search looks like a broken indexer.
#[tokio::test]
async fn an_empty_result_page_is_not_an_error() {
    let fetcher = Arc::new(ScriptedFetcher::serving(EMPTY_HTML));
    let releases = engine(fetcher, DEFINITION)
        .search(&terms("nothing-here"))
        .await
        .expect("an empty page is a valid response");
    assert!(releases.is_empty());
}

/// The query the engine builds is itself worth asserting. Definition-level
/// `search.inputs` were parsed away and discarded, so keyword searches silently
/// became unfiltered listing sweeps — the results looked plausible, just wrong.
#[tokio::test]
async fn the_keywords_actually_reach_the_tracker() {
    let fetcher = Arc::new(ScriptedFetcher::serving(ROWS_HTML));
    engine(Arc::clone(&fetcher), DEFINITION)
        .search(&terms("specific show"))
        .await
        .expect("search");

    let urls = fetcher.urls();
    assert!(
        urls.iter()
            .any(|u| u.contains("q=specific+show") || u.contains("q=specific%20show")),
        "the keyword must be on the wire: {urls:?}"
    );
}
