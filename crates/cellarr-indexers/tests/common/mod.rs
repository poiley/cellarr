//! A tracker that misbehaves on purpose.
//!
//! The replay fetchers elsewhere in these tests answer every request with the same
//! recorded page. Real trackers do not: they rate-limit after a while, fail only
//! for one awkward combination of query parameters, serve an interstitial instead
//! of content, and go down mid-search. Every indexer bug worth fixing has been one
//! of those, and none of them can be expressed by a fixture that never changes.
//!
//! [`ScriptedFetcher`] is the [`Fetcher`] seam with that behaviour attached: it
//! answers per-URL, can be told to start failing after N requests or whenever a
//! request matches a substring, and records everything it was asked for so a test
//! can assert on the requests themselves rather than only on the parsed result.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use cellarr_indexers::http::Fetcher;
use cellarr_indexers::{IndexerError, Result};

/// What the tracker does when asked.
#[derive(Debug, Clone)]
pub enum Reply {
    /// Serve this body.
    Body(String),
    /// Answer with an HTTP status, as a tracker refusing the request does.
    /// `429` is the rate-limit case, `403`/`522` the blocked/origin-down ones.
    Status(u16),
    /// Fail the way a request that never completes does.
    Timeout,
}

impl Reply {
    fn into_result(self) -> Result<String> {
        match self {
            Reply::Body(b) => Ok(b),
            Reply::Status(status) => Err(IndexerError::Status {
                status,
                body_snippet: Some(format!("scripted status {status}")),
            }),
            Reply::Timeout => Err(IndexerError::Parse("request timed out".to_string())),
        }
    }
}

/// One recorded request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// The URL asked for.
    pub url: String,
    /// The form body, for a POST.
    pub body: Option<String>,
}

/// A tracker whose behaviour a test scripts up front.
pub struct ScriptedFetcher {
    /// Consumed in order; once empty the fetcher falls through to the rules below.
    script: Mutex<VecDeque<Reply>>,
    /// `(url-substring, reply)` — the first match wins. This is how a tracker that
    /// only breaks on one query-parameter combination is expressed.
    rules: Vec<(String, Reply)>,
    /// Served when nothing else matches.
    default: Reply,
    /// Start failing with this once `limit` requests have been served, the way a
    /// tracker's rate limiter does.
    rate_limit: Option<(usize, Reply)>,
    seen: Mutex<Vec<Seen>>,
}

impl ScriptedFetcher {
    /// A tracker that always serves `body`.
    pub fn serving(body: impl Into<String>) -> Self {
        Self {
            script: Mutex::new(VecDeque::new()),
            rules: Vec::new(),
            default: Reply::Body(body.into()),
            rate_limit: None,
            seen: Mutex::new(Vec::new()),
        }
    }

    /// Answer `reply` whenever the requested URL contains `needle`.
    #[must_use]
    pub fn when_url_contains(mut self, needle: impl Into<String>, reply: Reply) -> Self {
        self.rules.push((needle.into(), reply));
        self
    }

    /// Answer these replies, in order, before anything else applies.
    #[must_use]
    pub fn then(mut self, replies: impl IntoIterator<Item = Reply>) -> Self {
        self.script.get_mut().expect("lock").extend(replies);
        self
    }

    /// Serve normally for `limit` requests, then answer `reply` forever after.
    ///
    /// This is the shape that matters: a limit that only bites part-way through a
    /// search is invisible to a fixture that answers every request identically.
    #[must_use]
    pub fn rate_limited_after(mut self, limit: usize, reply: Reply) -> Self {
        self.rate_limit = Some((limit, reply));
        self
    }

    /// Every request the fetcher was asked for, in order.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("lock").clone()
    }

    /// How many requests it has answered.
    pub fn count(&self) -> usize {
        self.seen.lock().expect("lock").len()
    }

    /// The URLs it was asked for.
    pub fn urls(&self) -> Vec<String> {
        self.seen().into_iter().map(|s| s.url).collect()
    }

    fn answer(&self, url: &str, body: Option<String>) -> Result<String> {
        let served = {
            let mut seen = self.seen.lock().expect("lock");
            seen.push(Seen {
                url: url.to_string(),
                body,
            });
            seen.len()
        };

        // The rate limit is checked first: once a tracker is refusing, it refuses
        // regardless of what was asked for.
        if let Some((limit, reply)) = &self.rate_limit {
            if served > *limit {
                return reply.clone().into_result();
            }
        }
        if let Some(reply) = self.script.lock().expect("lock").pop_front() {
            return reply.into_result();
        }
        for (needle, reply) in &self.rules {
            if url.contains(needle.as_str()) {
                return reply.clone().into_result();
            }
        }
        self.default.clone().into_result()
    }
}

#[async_trait]
impl Fetcher for ScriptedFetcher {
    async fn get(&self, url: &str) -> Result<String> {
        self.answer(url, None)
    }

    async fn post(&self, url: &str, body: &str, _content_type: &str) -> Result<String> {
        self.answer(url, Some(body.to_string()))
    }
}

/// A minimal Cardigann definition over [`ROWS_HTML`], used by the fault tests.
///
/// Deliberately plain: these tests are about how the engine behaves when the
/// *tracker* misbehaves, so the definition itself must not be the variable.
pub const DEFINITION: &str = r#"
id: faulttracker
name: Fault Tracker
type: public
encoding: UTF-8
links:
  - https://faulttracker.example/
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
    selector: "table.torrents > tbody > tr"
  fields:
    title:
      selector: a.title
    details:
      selector: a.title
      attribute: href
    download:
      selector: a.dl
      attribute: href
    size:
      selector: td.size
    seeders:
      selector: td.seeders
"#;

/// Two rows, with the CSS-hidden labels real trackers put in their cells.
pub const ROWS_HTML: &str = r#"
<html><body><table class="torrents"><tbody>
  <tr>
    <td><a class="title" href="/t/example-s01e01-1080p-11/">Example S01E01 1080p WEB</a>
        <a class="dl" href="magnet:?xt=urn:btih:1111111111111111111111111111111111111111">dl</a></td>
    <td class="size"><span>Size</span> 2.1 GB</td>
    <td class="seeders"><span>Seeds</span> 42</td>
  </tr>
  <tr>
    <td><a class="title" href="/t/example-s01e02-1080p-22/">Example S01E02 1080p WEB</a>
        <a class="dl" href="magnet:?xt=urn:btih:2222222222222222222222222222222222222222">dl</a></td>
    <td class="size"><span>Size</span> 2.2 GB</td>
    <td class="seeders"><span>Seeds</span> 7</td>
  </tr>
</tbody></table></body></html>
"#;

/// An empty result page, for the "this query returns nothing" case.
pub const EMPTY_HTML: &str =
    r#"<html><body><table class="torrents"><tbody></tbody></table></body></html>"#;
