//! Record/replay tests for the Cardigann engine.
//!
//! The definition and the search-results page are synthetic fixtures (see
//! `tests/fixtures/NOTES.md`). We parse the definition, build the search request,
//! and run its CSS `rows`/`fields` selectors + filter chain over the recorded HTML
//! served by a [`ReplayFetcher`], asserting normalization into `Release` — no
//! tracker is contacted.

use std::sync::Arc;

use async_trait::async_trait;
use cellarr_core::{Indexer, IndexerId, Protocol, SearchTerms};
use cellarr_indexers::cardigann::CardigannIndexer;
use cellarr_indexers::http::Fetcher;
use cellarr_indexers::{Definition, HostRateLimiter, Result};

const DEFINITION: &str = include_str!("fixtures/cardigann_mytracker.yml");
const RESULTS_HTML: &str = include_str!("fixtures/cardigann_mytracker.html");

/// A fetcher that replays the recorded results page for any request and records the
/// GET URLs and POST `(url, body)` pairs, so a test can assert what the engine built.
struct ReplayFetcher {
    body: &'static str,
    requested: std::sync::Mutex<Vec<String>>,
    posted: std::sync::Mutex<Vec<(String, String)>>,
}

impl ReplayFetcher {
    fn new(body: &'static str) -> Self {
        Self {
            body,
            requested: std::sync::Mutex::new(Vec::new()),
            posted: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Fetcher for ReplayFetcher {
    async fn get(&self, url: &str) -> Result<String> {
        self.requested.lock().expect("lock").push(url.to_string());
        Ok(self.body.to_string())
    }

    async fn post(&self, url: &str, body: &str, _content_type: &str) -> Result<String> {
        self.posted
            .lock()
            .expect("lock")
            .push((url.to_string(), body.to_string()));
        Ok(self.body.to_string())
    }
}

#[test]
fn parses_definition_metadata_and_caps() {
    let def = Definition::from_yaml(DEFINITION).expect("parse definition");
    assert_eq!(def.id, "mytracker");
    assert_eq!(def.name, "My Example Tracker");
    assert!(def.has_mode("search"));
    assert!(def.has_mode("tv-search"));
    assert!(def.has_mode("movie-search"));
    assert_eq!(def.caps.categorymappings.len(), 3);
    // Category mappings are read from the definition, never hardcoded.
    assert_eq!(def.torznab_category("10"), Some("5040"));
    assert_eq!(def.torznab_category("999"), None);
}

#[test]
fn extracts_releases_from_recorded_html() {
    let def = Definition::from_yaml(DEFINITION).expect("parse definition");
    let id = IndexerId::new();
    let engine = CardigannIndexer::new(id, def);

    let releases = engine.extract(RESULTS_HTML).expect("extract");

    // The non-torrent advertisement row must be ignored by the row selector.
    assert_eq!(releases.len(), 2, "only torrent rows are extracted");

    let first = &releases[0];
    assert_eq!(first.indexer_id, id);
    assert_eq!(first.protocol, Protocol::Torrent);
    assert_eq!(first.title, "Example.Show.S01E01.1080p.WEB-DL.H264-GROUP");
    // Relative hrefs are resolved against the definition's site link.
    assert_eq!(
        first.download_url,
        "https://mytracker.example/download.php?id=501&authkey=xyz"
    );
    assert_eq!(
        first.guid.as_deref(),
        Some("https://mytracker.example/details.php?id=501")
    );
    // "1.5 GB" parsed to bytes.
    assert_eq!(first.size, Some((1.5 * 1024.0 * 1024.0 * 1024.0) as u64));
    // "Seeders: 88" reduced to 88 by the field's regexp filter.
    assert_eq!(first.seeders, Some(88));

    let second = &releases[1];
    assert_eq!(second.title, "Example.Show.S01E02.720p.HDTV.x264-OTHER");
    assert_eq!(second.size, Some(700 * 1024 * 1024));
    assert_eq!(second.seeders, Some(12));
}

#[tokio::test]
async fn search_builds_request_and_extracts() {
    let def = Definition::from_yaml(DEFINITION).expect("parse definition");
    let fetcher = Arc::new(ReplayFetcher::new(RESULTS_HTML));
    let engine = CardigannIndexer::with_deps(
        IndexerId::new(),
        def,
        std::collections::BTreeMap::new(),
        fetcher.clone(),
        Arc::new(HostRateLimiter::conservative_default()),
    );

    let terms = SearchTerms {
        queries: vec!["Example Show".to_string()],
        ids: vec![],
        numbering: vec![],
        categories: vec![],
    };
    let releases = engine.search(&terms).await.expect("search");

    // The request was built from the definition's path + templated `q` input.
    let requested = fetcher.requested.lock().expect("lock").clone();
    assert_eq!(requested.len(), 1, "one configured path -> one request");
    let url = &requested[0];
    assert!(
        url.starts_with("https://mytracker.example/torrents.php?"),
        "{url}"
    );
    assert!(url.contains("q=Example+Show"), "templated keywords: {url}");

    // And the recorded page normalized into releases the same way.
    assert_eq!(releases.len(), 2);
    assert_eq!(
        releases[0].title,
        "Example.Show.S01E01.1080p.WEB-DL.H264-GROUP"
    );
    assert_eq!(releases[0].seeders, Some(88));
    assert!(releases[0]
        .download_url
        .starts_with("https://mytracker.example/"));
}

#[tokio::test]
async fn search_via_post_sends_a_form_body() {
    const POST_DEF: &str = r#"
id: postt
name: Post Tracker
links: [https://post.example/]
search:
  paths:
    - path: /search.php
      method: post
      inputs:
        q: "{{ .Keywords }}"
        cat: "tv"
  rows:
    selector: "table.torrents > tbody > tr.torrent"
  fields:
    title: { selector: "a.title" }
    download: { selector: "a.download", attribute: href }
"#;
    let def = Definition::from_yaml(POST_DEF).expect("parse");
    let fetcher = Arc::new(ReplayFetcher::new(RESULTS_HTML));
    let engine = CardigannIndexer::with_deps(
        IndexerId::new(),
        def,
        std::collections::BTreeMap::new(),
        fetcher.clone(),
        Arc::new(HostRateLimiter::conservative_default()),
    );
    let terms = SearchTerms {
        queries: vec!["Example Show".into()],
        ..Default::default()
    };
    let releases = engine.search(&terms).await.expect("search");

    // A POST path issues no GET; one POST with the inputs as a urlencoded form body
    // carried in the body (not the URL).
    assert!(
        fetcher.requested.lock().unwrap().is_empty(),
        "a POST path issues no GET"
    );
    let posted = fetcher.posted.lock().unwrap().clone();
    assert_eq!(posted.len(), 1, "one configured path -> one POST");
    let (url, body) = &posted[0];
    assert_eq!(url, "https://post.example/search.php");
    assert!(body.contains("q=Example+Show"), "form body: {body}");
    assert!(body.contains("cat=tv"), "form body: {body}");
    assert_eq!(
        releases.len(),
        2,
        "recorded page still normalizes to releases"
    );
}
