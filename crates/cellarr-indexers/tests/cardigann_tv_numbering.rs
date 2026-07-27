//! Season/episode has to reach the tracker one way or another.
//!
//! A scrape-style tracker often has no season/episode URL parameter: its
//! definition declares `tv-search` in `caps` but templates only `q`. The engine
//! computes the numbering regardless, so if nothing carries it the search silently
//! degrades to a bare series-title query — every season, thousands of rows, the
//! wanted episode nowhere near the top. These tests pin that such a definition gets
//! the numbering folded into the keyword, and that a definition which templates the
//! numbering itself is left alone.
//!
//! Fixtures are synthetic (see `tests/fixtures/NOTES.md`); no tracker is contacted.

use std::sync::Arc;

use async_trait::async_trait;
use cellarr_core::{Indexer, IndexerId, SearchTerms};
use cellarr_indexers::cardigann::CardigannIndexer;
use cellarr_indexers::http::Fetcher;
use cellarr_indexers::{Definition, HostRateLimiter, Result};

/// A tracker with no season/episode parameter — only a keyword box.
const KEYWORD_ONLY: &str = r#"
id: keywordonly
name: Keyword Only Tracker
type: public
links:
  - https://keywordonly.example/
caps:
  categorymappings:
    - { id: "10", cat: "5040", desc: "TV/HD" }
  modes:
    search: [q]
    tv-search: [q, season, ep]
search:
  paths:
    - path: /browse/
  inputs:
    q: "{{ .Keywords }}"
  rows:
    selector: tr
  fields:
    title:
      selector: a
    download:
      selector: a
      attribute: href
"#;

/// A tracker that takes season/episode as real parameters.
const NATIVE_NUMBERING: &str = r#"
id: nativenumbering
name: Native Numbering Tracker
type: public
links:
  - https://native.example/
caps:
  categorymappings:
    - { id: "10", cat: "5040", desc: "TV/HD" }
  modes:
    search: [q]
    tv-search: [q, season, ep]
search:
  paths:
    - path: /browse/
  inputs:
    q: "{{ .Keywords }}"
    season: "{{ .Query.Season }}"
    ep: "{{ .Query.Episode }}"
  rows:
    selector: tr
  fields:
    title:
      selector: a
    download:
      selector: a
      attribute: href
"#;

struct RecordingFetcher(std::sync::Mutex<Vec<String>>);

#[async_trait]
impl Fetcher for RecordingFetcher {
    async fn get(&self, url: &str) -> Result<String> {
        self.0.lock().expect("lock").push(url.to_string());
        Ok("<html><body><table></table></body></html>".to_string())
    }
}

async fn search_url(definition: &str, terms: SearchTerms) -> String {
    let fetcher = Arc::new(RecordingFetcher(std::sync::Mutex::new(Vec::new())));
    let def = Definition::from_yaml(definition).expect("parse definition");
    CardigannIndexer::with_deps(
        IndexerId::new(),
        def,
        std::collections::BTreeMap::new(),
        Arc::clone(&fetcher) as Arc<dyn Fetcher>,
        Arc::new(HostRateLimiter::conservative_default()),
    )
    .search(&terms)
    .await
    .expect("search");
    let urls = fetcher.0.lock().expect("lock").clone();
    urls.into_iter().next().expect("a request was issued")
}

fn episode_terms(season: &str, episode: &str) -> SearchTerms {
    SearchTerms {
        queries: vec!["Love Island".to_string()],
        numbering: vec![
            ("season".to_string(), season.to_string()),
            ("ep".to_string(), episode.to_string()),
        ],
        ..SearchTerms::default()
    }
}

#[tokio::test]
async fn a_keyword_only_tracker_gets_the_episode_folded_into_the_query() {
    let url = search_url(KEYWORD_ONLY, episode_terms("6", "3")).await;
    assert!(
        url.contains("q=Love+Island+S06E03") || url.contains("q=Love%20Island%20S06E03"),
        "episode should reach the tracker in the keyword: {url}"
    );
}

#[tokio::test]
async fn a_season_without_an_episode_folds_in_as_a_season_search() {
    let terms = SearchTerms {
        queries: vec!["Love Island".to_string()],
        numbering: vec![("season".to_string(), "6".to_string())],
        ..SearchTerms::default()
    };
    let url = search_url(KEYWORD_ONLY, terms).await;
    assert!(
        url.contains("S06"),
        "season should reach the tracker: {url}"
    );
    assert!(!url.contains("S06E"), "no episode was asked for: {url}");
}

#[tokio::test]
async fn a_tracker_with_real_numbering_parameters_is_left_alone() {
    let url = search_url(NATIVE_NUMBERING, episode_terms("6", "3")).await;
    assert!(
        url.contains("season=6") && url.contains("ep=3"),
        "native parameters should still be sent: {url}"
    );
    assert!(
        !url.contains("S06E03"),
        "numbering must not also be folded into the keyword: {url}"
    );
}

#[tokio::test]
async fn a_search_with_no_numbering_is_unchanged() {
    let terms = SearchTerms {
        queries: vec!["Sinners 2025".to_string()],
        ..SearchTerms::default()
    };
    let url = search_url(KEYWORD_ONLY, terms).await;
    assert!(url.contains("q=Sinners+2025"), "{url}");
    assert!(!url.contains("S0"), "nothing to fold in: {url}");
}
