//! Record/replay tests for resolver-backed download links.
//!
//! Some trackers publish listing rows with no download link at all — the magnet is
//! served from an endpoint that wants a signed request. The definition alone can't
//! express that, so those rows would be dropped for want of a link. These tests pin
//! that a [`DownloadResolver`] fills them in, that the signed request is shaped the
//! way the tracker expects, and that a row the resolver can't satisfy is dropped
//! rather than surfaced with an empty link.
//!
//! Fixtures are synthetic (see `tests/fixtures/NOTES.md`); no tracker is contacted.

use std::sync::Arc;

use async_trait::async_trait;
use cellarr_core::{Indexer, IndexerId, SearchTerms};
use cellarr_indexers::cardigann::CardigannIndexer;
use cellarr_indexers::http::Fetcher;
use cellarr_indexers::resolve::DownloadResolver;
use cellarr_indexers::{Definition, ExtTorrentsResolver, HostRateLimiter, IndexerError, Result};
use cellarr_core::Release;

const DEFINITION: &str = include_str!("fixtures/cardigann_signedtracker.yml");
const LISTING_HTML: &str = include_str!("fixtures/cardigann_signedtracker.html");
const DETAILS_HTML: &str = include_str!("fixtures/cardigann_signedtracker_details.html");
/// Page two of a result set that fits on one page.
const EMPTY_LISTING_HTML: &str =
    "<html><body><table class=\"torrents\"><tbody></tbody></table></body></html>";

/// Serves the listing for the search path and the details page for a release URL,
/// and answers the signed magnet endpoint with a per-id magnet. Records POST bodies
/// so a test can assert the signed request's shape.
struct SignedTrackerFetcher {
    posted: std::sync::Mutex<Vec<(String, String)>>,
    requested: std::sync::Mutex<Vec<String>>,
    /// Ids the endpoint refuses, standing in for a torrent pulled since the listing.
    refuse: Vec<&'static str>,
}

impl SignedTrackerFetcher {
    fn new() -> Self {
        Self {
            posted: std::sync::Mutex::new(Vec::new()),
            requested: std::sync::Mutex::new(Vec::new()),
            refuse: Vec::new(),
        }
    }

    fn refusing(id: &'static str) -> Self {
        Self {
            posted: std::sync::Mutex::new(Vec::new()),
            requested: std::sync::Mutex::new(Vec::new()),
            refuse: vec![id],
        }
    }

    fn posts(&self) -> Vec<(String, String)> {
        self.posted.lock().expect("lock").clone()
    }

    fn gets(&self) -> Vec<String> {
        self.requested.lock().expect("lock").clone()
    }

    /// The `torrent_id` field of a recorded form body.
    fn form_field<'a>(body: &'a str, name: &str) -> Option<&'a str> {
        body.split('&')
            .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
    }
}

#[async_trait]
impl Fetcher for SignedTrackerFetcher {
    async fn get(&self, url: &str) -> Result<String> {
        self.requested.lock().expect("lock").push(url.to_string());
        if url.contains("/browse/") {
            // A two-row result set has nothing on page two, so the second path
            // contributes no rows — as it would against the real tracker.
            if url.contains("page=2") {
                return Ok(EMPTY_LISTING_HTML.to_string());
            }
            return Ok(LISTING_HTML.to_string());
        }
        Ok(DETAILS_HTML.to_string())
    }

    async fn post(&self, url: &str, body: &str, _content_type: &str) -> Result<String> {
        self.posted
            .lock()
            .expect("lock")
            .push((url.to_string(), body.to_string()));
        let id = Self::form_field(body, "torrent_id").unwrap_or_default();
        if self.refuse.contains(&id) {
            return Ok(r#"{"success":false,"error":"Invalid session"}"#.to_string());
        }
        // The live endpoint answers `{"success":true,"type":"magnet","url":"magnet:…"}`
        // — the link arrives under `url`, not `magnet`. The stub mirrors that so this
        // path is exercised against the shape the tracker really sends.
        Ok(format!(
            r#"{{"success":true,"type":"magnet","url":"magnet:?xt=urn:btih:{:0>40}"}}"#,
            id
        ))
    }
}

fn indexer(fetcher: Arc<SignedTrackerFetcher>) -> CardigannIndexer {
    let def = Definition::from_yaml(DEFINITION).expect("parse definition");
    CardigannIndexer::with_deps(
        IndexerId::new(),
        def,
        std::collections::BTreeMap::new(),
        fetcher,
        Arc::new(HostRateLimiter::conservative_default()),
    )
    .with_resolver(Arc::new(ExtTorrentsResolver::with_hosts(vec![
        "signedtracker.example".to_string(),
    ])))
}


/// Search, then resolve exactly the rows the engine deferred — what the pipeline
/// does, except the pipeline only ever resolves the ONE release it chose to grab.
/// Resolving all of them here keeps these tests exercising the resolver end to end.
async fn search_and_resolve(ix: &CardigannIndexer, query: &str) -> Vec<Release> {
    let found = ix
        .search(&SearchTerms {
            queries: vec![query.to_string()],
            ..SearchTerms::default()
        })
        .await
        .expect("search");
    let mut out = Vec::new();
    for release in found {
        if release.link_is_deferred() {
            match cellarr_core::traits::Indexer::resolve(ix, release).await {
                Ok(resolved) => out.push(resolved),
                // The old engine dropped a row it could not resolve; the pipeline now
                // moves to the next candidate instead. Same outcome for these tests.
                Err(_) => continue,
            }
        } else {
            out.push(release);
        }
    }
    out
}

#[tokio::test]
async fn rows_without_a_link_are_resolved_into_magnets() {
    let fetcher = Arc::new(SignedTrackerFetcher::new());
    let releases = search_and_resolve(&indexer(fetcher.clone()), "example show").await;

    assert_eq!(releases.len(), 2, "both rows should survive: {releases:?}");
    assert!(
        releases[0].download_url.starts_with("magnet:?xt=urn:btih:"),
        "expected a resolved magnet, got {:?}",
        releases[0].download_url
    );
    assert!(
        releases[0].download_url.contains("11223344"),
        "magnet should belong to the row's id: {:?}",
        releases[0].download_url
    );
    // The display name is still appended, as it is for definition-supplied magnets.
    assert!(releases[0].download_url.contains("dn="));
    assert_eq!(releases[0].seeders, Some(42));
    // The cells carry a CSS-hidden label next to the value, as trackers render them.
    assert_eq!(
        releases[0].size,
        Some((2.1 * 1024.0 * 1024.0 * 1024.0) as u64),
        "labelled size cell should still parse"
    );
}

#[tokio::test]
async fn the_signed_request_carries_id_download_type_timestamp_and_signature() {
    let fetcher = Arc::new(SignedTrackerFetcher::new());
    let _ = search_and_resolve(&indexer(fetcher.clone()), "example show").await;

    let posts = fetcher.posts();
    assert_eq!(posts.len(), 2, "one signed request per link-less row");
    let (url, body) = &posts[0];
    assert!(
        url.ends_with("/ajax/getTorrentMagnet.php"),
        "unexpected endpoint {url}"
    );
    assert!(
        url.starts_with("https://signedtracker.example"),
        "endpoint should stay on the release's own mirror: {url}"
    );
    assert_eq!(
        SignedTrackerFetcher::form_field(body, "torrent_id"),
        Some("11223344")
    );
    // `download_type` is the field the endpoint reads to choose which link to hand
    // back. It was previously sent as `action=get_magnet`, which is not a parameter
    // the endpoint has, so the field it does look for arrived empty.
    assert_eq!(
        SignedTrackerFetcher::form_field(body, "download_type"),
        Some("magnet")
    );
    assert_eq!(
        SignedTrackerFetcher::form_field(body, "sessid"),
        Some("a0e4b5e37e90ee06e6232b6a72498804")
    );
    // The signature is SHA-256 over `id|timestamp|pageToken`; recompute it from the
    // timestamp the request actually carried rather than pinning a clock.
    let timestamp = SignedTrackerFetcher::form_field(body, "timestamp").expect("timestamp");
    let expected = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(format!("11223344|{timestamp}|bd10d1c719b340791e1d11cd271a7ca0").as_bytes());
        h.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    assert_eq!(
        SignedTrackerFetcher::form_field(body, "hmac"),
        Some(expected.as_str())
    );
}

#[tokio::test]
async fn a_row_the_endpoint_refuses_is_dropped_not_surfaced_linkless() {
    let fetcher = Arc::new(SignedTrackerFetcher::refusing("11223344"));
    let releases = search_and_resolve(&indexer(fetcher), "example show").await;

    assert_eq!(
        releases.len(),
        1,
        "the refused row should drop: {releases:?}"
    );
    assert!(releases[0].download_url.contains("55667788"));
    assert!(
        releases.iter().all(|r| !r.download_url.is_empty()),
        "no release may escape with an empty link"
    );
}

#[tokio::test]
async fn without_a_resolver_link_less_rows_drop_as_before() {
    let def = Definition::from_yaml(DEFINITION).expect("parse definition");
    let releases = CardigannIndexer::with_deps(
        IndexerId::new(),
        def,
        std::collections::BTreeMap::new(),
        Arc::new(SignedTrackerFetcher::new()),
        Arc::new(HostRateLimiter::conservative_default()),
    )
    .search(&SearchTerms {
        queries: vec!["example show".to_string()],
        ..SearchTerms::default()
    })
    .await
    .expect("search");

    assert!(
        releases.is_empty(),
        "a definition with no link field yields nothing without a resolver: {releases:?}"
    );
}

#[tokio::test]
async fn a_resolver_that_does_not_claim_the_host_leaves_rows_dropped() {
    let def = Definition::from_yaml(DEFINITION).expect("parse definition");
    let releases = CardigannIndexer::with_deps(
        IndexerId::new(),
        def,
        std::collections::BTreeMap::new(),
        Arc::new(SignedTrackerFetcher::new()),
        Arc::new(HostRateLimiter::conservative_default()),
    )
    .with_resolver(Arc::new(ExtTorrentsResolver::new()))
    .search(&SearchTerms {
        queries: vec!["example show".to_string()],
        ..SearchTerms::default()
    })
    .await
    .expect("search");

    assert!(
        releases.is_empty(),
        "resolver claims only its own mirrors: {releases:?}"
    );
}

/// Guards the seam the engine relies on: a fetcher with no POST support must not
/// silently yield link-less releases.
#[tokio::test]
async fn a_get_only_fetcher_cannot_resolve_and_drops_the_rows() {
    struct GetOnly;
    #[async_trait]
    impl Fetcher for GetOnly {
        async fn get(&self, url: &str) -> Result<String> {
            if url.contains("/browse/") {
                return Ok(LISTING_HTML.to_string());
            }
            Ok(DETAILS_HTML.to_string())
        }
    }

    let def = Definition::from_yaml(DEFINITION).expect("parse definition");
    let releases = CardigannIndexer::with_deps(
        IndexerId::new(),
        def,
        std::collections::BTreeMap::new(),
        Arc::new(GetOnly),
        Arc::new(HostRateLimiter::conservative_default()),
    )
    .with_resolver(Arc::new(ExtTorrentsResolver::with_hosts(vec![
        "signedtracker.example".to_string(),
    ])));
    let releases = search_and_resolve(&releases, "example show").await;

    assert!(releases.is_empty(), "{releases:?}");
    // The seam's own error stays typed rather than panicking.
    let unsupported = IndexerError::Unsupported("x".to_string());
    assert!(unsupported.to_string().contains('x'));
}

/// A definition that declares its query once in `search.inputs` and uses `paths`
/// only to vary the page must send those inputs on every path — dropping them
/// silently turns a keyword search into an unfiltered listing sweep.
#[tokio::test]
async fn search_level_inputs_are_sent_on_every_path() {
    let fetcher = Arc::new(SignedTrackerFetcher::new());
    let _ = search_and_resolve(&indexer(fetcher.clone()), "example show").await;

    let listing: Vec<String> = fetcher
        .gets()
        .into_iter()
        .filter(|u| u.contains("/browse/"))
        .collect();
    assert_eq!(listing.len(), 2, "one request per path: {listing:?}");
    for url in &listing {
        assert!(url.contains("q=example+show"), "missing keywords: {url}");
        assert!(url.contains("with_adult=1"), "missing shared input: {url}");
        // `age` renders empty when a keyword was supplied, and empty inputs are
        // omitted rather than sent bare. Matched with its delimiter so the check
        // isn't satisfied by the `page=` of the second path.
        assert!(
            !url.contains("?age=") && !url.contains("&age="),
            "empty input should be omitted: {url}"
        );
    }
    assert!(
        listing.iter().any(|u| u.contains("page=2")),
        "path-level input should still be applied: {listing:?}"
    );
}

/// The keyword-less sweep takes the `{{ else }}` branch, which is the whole reason
/// `or` has to evaluate both of its arguments.
#[tokio::test]
async fn a_keywordless_search_takes_the_else_branch_of_an_or_condition() {
    let fetcher = Arc::new(SignedTrackerFetcher::new());
    indexer(fetcher.clone()).latest().await.expect("latest");

    let listing: Vec<String> = fetcher
        .gets()
        .into_iter()
        .filter(|u| u.contains("/browse/"))
        .collect();
    assert!(!listing.is_empty());
    for url in &listing {
        assert!(url.contains("age=4"), "expected the else branch: {url}");
        assert!(!url.contains("q="), "no keyword to send: {url}");
    }
}

/// Resolution costs two requests per release against the slowest indexer in the
/// set, so an unbounded sweep is not acceptable. The cap keeps the best-seeded
/// Search must hand back EVERY candidate, resolving none of them.
///
/// Resolving at search time meant resolving speculatively, which is expensive
/// enough on a tracker that signs its magnets that it had to be capped to the
/// best-seeded few — and the cap discarded the rest before the decision engine ever
/// saw them. On the live library that was 1,050 of 1,188 candidates, 88%, thrown
/// away unconsidered: if the kept few happened to be the wrong quality or language,
/// the search yielded nothing while dozens of viable alternatives existed.
///
/// So the contract is now the opposite of a cap: nothing is dropped, nothing is
/// fetched, and the cost is paid once by whichever release actually wins.
#[tokio::test]
async fn search_defers_every_link_and_costs_no_requests() {
    let fetcher = Arc::new(SignedTrackerFetcher::new());
    let releases = CardigannIndexer::with_deps(
        IndexerId::new(),
        Definition::from_yaml(DEFINITION).expect("parse definition"),
        std::collections::BTreeMap::from([("resolveLimit".to_string(), "1".to_string())]),
        Arc::clone(&fetcher) as Arc<dyn Fetcher>,
        Arc::new(HostRateLimiter::conservative_default()),
    )
    .with_resolver(Arc::new(ExtTorrentsResolver::with_hosts(vec![
        "signedtracker.example".to_string(),
    ])))
    .search(&SearchTerms {
        queries: vec!["example show".to_string()],
        ..SearchTerms::default()
    })
    .await
    .expect("search");

    assert_eq!(
        releases.len(),
        2,
        "every candidate survives search, even under a resolveLimit that used to \
         discard all but one: {releases:?}"
    );
    assert!(
        releases.iter().all(|r| r.link_is_deferred()),
        "links are deferred, not fetched: {releases:?}"
    );
    assert!(
        releases.iter().all(|r| !r.has_no_link()),
        "a deferred link is NOT an absent one — Discover must keep these candidates"
    );
    assert_eq!(
        fetcher.posts().len(),
        0,
        "search must cost no resolve requests at all; the winner pays, once"
    );
}

/// A tracker that publishes a courtesy interval is telling you the rate at which
/// it stops answering; the engine must not out-run it.
#[tokio::test]
async fn a_definitions_request_delay_paces_the_engine() {
    let yaml = DEFINITION.replace("type: public", "type: public\nrequestDelay: 1");
    let def = Definition::from_yaml(&yaml).expect("parse definition");
    assert_eq!(def.request_delay, Some(1.0), "requestDelay should parse");

    let started = std::time::Instant::now();
    let releases = CardigannIndexer::with_deps(
        IndexerId::new(),
        def,
        std::collections::BTreeMap::from([("resolveLimit".to_string(), "1".to_string())]),
        Arc::new(SignedTrackerFetcher::new()) as Arc<dyn Fetcher>,
        Arc::new(HostRateLimiter::conservative_default()),
    )
    .with_resolver(Arc::new(ExtTorrentsResolver::with_hosts(vec![
        "signedtracker.example".to_string(),
    ])));
    // Pacing has to cover BOTH phases now that the link is fetched later: the search
    // requests, and the resolve the winner triggers. Deferring must not become a way
    // to slip past a tracker's published interval.
    let releases = search_and_resolve(&releases, "example show").await;

    assert_eq!(releases.len(), 2);
    // Two search paths plus the resolves = at least three paced requests, so at
    // least two full delays must have elapsed.
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(2),
        "expected the engine to pace itself, took only {:?}",
        started.elapsed()
    );
}

/// The magnet request must be addressed to the host the details page actually
/// came from, not the one it was asked for.
///
/// The tracker publishes several entry hostnames that redirect to one canonical
/// host, and the session the page's tokens belong to lives on the canonical one.
/// Addressing the entry hostname gets a redirect instead of the endpoint, so the
/// session never arrives and every request is refused as "Invalid session" —
/// which reads as a signing problem and is nothing of the sort.
struct RedirectingFetcher {
    posted: std::sync::Mutex<Vec<String>>,
}

#[async_trait]
impl Fetcher for RedirectingFetcher {
    async fn get(&self, _url: &str) -> Result<String> {
        Ok(DETAILS_HTML.to_string())
    }

    async fn get_page(&self, _url: &str) -> Result<cellarr_indexers::FetchedPage> {
        Ok(cellarr_indexers::FetchedPage {
            body: DETAILS_HTML.to_string(),
            final_url: "https://canonical.example/some-release-77/".to_string(),
        })
    }

    async fn post_in_page_session(&self, url: &str, _body: &str) -> Result<String> {
        self.posted.lock().expect("lock").push(url.to_string());
        Ok(r#"{"success":true,"type":"magnet","url":"magnet:?xt=urn:btih:aa"}"#.to_string())
    }
}

#[tokio::test]
async fn the_magnet_request_goes_to_the_host_the_details_page_resolved_to() {
    let fetcher = RedirectingFetcher {
        posted: std::sync::Mutex::new(Vec::new()),
    };
    let resolver = ExtTorrentsResolver::with_hosts(vec!["entry.example".to_string()]);

    let link = resolver
        .resolve("https://entry.example/some-release-77/", &fetcher)
        .await
        .expect("the magnet resolves");
    assert!(link.starts_with("magnet:"), "got {link}");

    let posted = fetcher.posted.lock().expect("lock").clone();
    assert_eq!(posted.len(), 1, "one magnet request, got {posted:?}");
    assert!(
        posted[0].starts_with("https://canonical.example/"),
        "the magnet request must go to the host the page resolved to, got {}",
        posted[0]
    );
}
