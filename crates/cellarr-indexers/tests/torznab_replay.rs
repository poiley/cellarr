//! Record/replay tests for the native Torznab/Newznab adapters.
//!
//! No live indexer is contacted: a [`ReplayFetcher`] returns recorded fixture
//! bodies keyed on the `t=` mode in the request URL, and we assert the adapter
//! calls `t=caps` first and normalizes search results into `Release`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cellarr_core::{Indexer, IndexerId, Protocol, SearchTerms};
use cellarr_indexers::http::Fetcher;
use cellarr_indexers::{HostRateLimiter, NewznabIndexer, Result, TorznabIndexer};

const TORZNAB_CAPS: &str = include_str!("fixtures/torznab_caps.xml");
const TORZNAB_SEARCH: &str = include_str!("fixtures/torznab_search.xml");
const NEWZNAB_CAPS: &str = include_str!("fixtures/newznab_caps.xml");
const NEWZNAB_SEARCH: &str = include_str!("fixtures/newznab_search.xml");

/// A fetcher that replays recorded bodies and records the order of requests.
struct ReplayFetcher {
    caps_body: &'static str,
    search_body: &'static str,
    caps_calls: AtomicUsize,
    search_calls: AtomicUsize,
    requested_urls: std::sync::Mutex<Vec<String>>,
}

impl ReplayFetcher {
    fn new(caps_body: &'static str, search_body: &'static str) -> Self {
        Self {
            caps_body,
            search_body,
            caps_calls: AtomicUsize::new(0),
            search_calls: AtomicUsize::new(0),
            requested_urls: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Fetcher for ReplayFetcher {
    async fn get(&self, url: &str) -> Result<String> {
        self.requested_urls
            .lock()
            .expect("lock")
            .push(url.to_string());
        if url.contains("t=caps") {
            self.caps_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.caps_body.to_string())
        } else {
            self.search_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.search_body.to_string())
        }
    }
}

fn limiter() -> Arc<HostRateLimiter> {
    Arc::new(HostRateLimiter::conservative_default())
}

#[tokio::test]
async fn torznab_calls_caps_first_then_normalizes_search() {
    let fetcher = Arc::new(ReplayFetcher::new(TORZNAB_CAPS, TORZNAB_SEARCH));
    let id = IndexerId::new();
    let indexer = TorznabIndexer::with_deps(
        id,
        "Example Torznab",
        "https://tracker.example/api",
        Some("KEY".to_string()),
        fetcher.clone(),
        limiter(),
    )
    .expect("construct");

    // A TV search with a tvdbid + season/ep should pick the tvsearch mode.
    let terms = SearchTerms {
        queries: vec!["Example Show".to_string()],
        ids: vec![("tvdbid".to_string(), "12345".to_string())],
        numbering: vec![
            ("season".to_string(), "2".to_string()),
            ("ep".to_string(), "5".to_string()),
        ],
    };

    let releases = indexer.search(&terms).await.expect("search");

    // caps was fetched exactly once, before the search.
    assert_eq!(
        fetcher.caps_calls.load(Ordering::SeqCst),
        1,
        "caps fetched once"
    );
    assert_eq!(fetcher.search_calls.load(Ordering::SeqCst), 1);
    let urls = fetcher.requested_urls.lock().expect("lock").clone();
    assert!(
        urls[0].contains("t=caps"),
        "first request is t=caps: {}",
        urls[0]
    );
    assert!(
        urls[1].contains("t=tvsearch"),
        "tv terms select tvsearch: {}",
        urls[1]
    );
    // Only caps-advertised params are sent.
    assert!(urls[1].contains("tvdbid=12345"));
    assert!(urls[1].contains("season=2"));
    assert!(urls[1].contains("ep=5"));
    assert!(urls[1].contains("apikey=KEY"));

    // Normalization into Release.
    assert_eq!(releases.len(), 2);
    let first = &releases[0];
    assert_eq!(first.indexer_id, id);
    assert_eq!(first.protocol, Protocol::Torrent);
    assert_eq!(
        first.title,
        "Example.Show.S02E05.1080p.WEB-DL.DD5.1.H.264-GROUP"
    );
    assert!(first.download_url.starts_with("magnet:?"));
    assert_eq!(first.size, Some(2_147_483_648));
    assert_eq!(first.seeders, Some(123));
    assert_eq!(
        first.guid.as_deref(),
        Some("https://tracker.example/details?id=111")
    );
    // downloadvolumefactor=0 -> freeleech flag.
    assert!(first.indexer_flags.contains(&"freeleech".to_string()));

    // Second item uses the enclosure .torrent URL and a partial-freeleech flag.
    let second = &releases[1];
    assert_eq!(
        second.download_url,
        "https://tracker.example/download/112.torrent"
    );
    assert!(second
        .indexer_flags
        .contains(&"partial-freeleech".to_string()));
}

#[tokio::test]
async fn caps_is_cached_across_searches() {
    let fetcher = Arc::new(ReplayFetcher::new(TORZNAB_CAPS, TORZNAB_SEARCH));
    let indexer = TorznabIndexer::with_deps(
        IndexerId::new(),
        "Example",
        "https://tracker.example/api",
        None,
        fetcher.clone(),
        limiter(),
    )
    .expect("construct");

    let terms = SearchTerms {
        queries: vec!["q".to_string()],
        ids: vec![],
        numbering: vec![],
    };
    indexer.search(&terms).await.expect("search 1");
    indexer.search(&terms).await.expect("search 2");

    assert_eq!(
        fetcher.caps_calls.load(Ordering::SeqCst),
        1,
        "t=caps fetched once and cached"
    );
    assert_eq!(fetcher.search_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn newznab_normalizes_usenet_results() {
    let fetcher = Arc::new(ReplayFetcher::new(NEWZNAB_CAPS, NEWZNAB_SEARCH));
    let id = IndexerId::new();
    let indexer = NewznabIndexer::with_deps(
        id,
        "Example Newznab",
        "https://news.example/api",
        Some("KEY".to_string()),
        fetcher.clone(),
        limiter(),
    )
    .expect("construct");

    // A movie search by imdbid should select the movie mode.
    let terms = SearchTerms {
        queries: vec!["Example Movie".to_string()],
        ids: vec![("imdbid".to_string(), "tt1234567".to_string())],
        numbering: vec![],
    };
    let releases = indexer.search(&terms).await.expect("search");

    let urls = fetcher.requested_urls.lock().expect("lock").clone();
    assert!(urls[0].contains("t=caps"));
    assert!(
        urls[1].contains("t=movie"),
        "imdbid selects movie mode: {}",
        urls[1]
    );

    assert_eq!(releases.len(), 2);
    assert!(releases.iter().all(|r| r.protocol == Protocol::Usenet));
    assert!(releases[0].download_url.ends_with(".nzb&apikey=KEY"));
    assert_eq!(releases[0].size, Some(2_469_606_195));
    // Usenet results carry no seeders.
    assert_eq!(releases[0].seeders, None);
}

#[tokio::test]
async fn latest_uses_plain_search_without_query() {
    let fetcher = Arc::new(ReplayFetcher::new(TORZNAB_CAPS, TORZNAB_SEARCH));
    let indexer = TorznabIndexer::with_deps(
        IndexerId::new(),
        "Example",
        "https://tracker.example/api",
        None,
        fetcher.clone(),
        limiter(),
    )
    .expect("construct");

    let releases = indexer.latest().await.expect("latest");
    assert_eq!(releases.len(), 2);
    let urls = fetcher.requested_urls.lock().expect("lock").clone();
    assert!(urls[0].contains("t=caps"));
    assert!(urls[1].contains("t=search"));
    assert!(
        !urls[1].contains("q="),
        "latest issues no query: {}",
        urls[1]
    );
}
