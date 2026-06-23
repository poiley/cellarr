//! Shared record/replay harness for the download-client contract tests.
//!
//! Loads a fixture (an ordered list of expected-request → response exchanges,
//! see `tests/fixtures/README.md`), wires it in as a [`HttpTransport`], and
//! asserts each request the adapter makes matches the next fixture exchange. No
//! live download client is contacted.

// This module is included by several test binaries; each compiles it separately
// and uses only the helpers it needs, so a helper unused by one binary is not
// dead code overall.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use cellarr_core::release::Protocol;
use cellarr_core::{
    ContentRef, Coordinates, DownloadClientId, GrabRequest, IndexerId, LibraryId, MediaType,
    Release,
};
use cellarr_download::error::DownloadError;
use cellarr_download::http::{HttpRequest, HttpResponse, HttpTransport};
use serde_json::Value;

/// One expected-request/response pair from a fixture.
struct Exchange {
    expect: Value,
    response: HttpResponse,
}

/// A transport that replays fixture exchanges in order, asserting requests.
pub struct ReplayTransport {
    exchanges: Mutex<std::collections::VecDeque<Exchange>>,
    label: String,
}

impl ReplayTransport {
    /// Load a fixture by path relative to the crate's `tests/fixtures` dir.
    pub fn load(relative: &str) -> Self {
        let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), relative);
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {path}: {e}"));
        let doc: Value =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse fixture {path}: {e}"));
        let exchanges = doc["exchanges"]
            .as_array()
            .unwrap_or_else(|| panic!("fixture {path} has no exchanges array"))
            .iter()
            .map(|ex| Exchange {
                expect: ex["expect"].clone(),
                response: parse_response(&ex["response"]),
            })
            .collect();
        Self {
            exchanges: Mutex::new(exchanges),
            label: relative.to_string(),
        }
    }

    /// Assert all exchanges were consumed (no unsent fixture remains).
    pub fn assert_drained(&self) {
        let remaining = self.exchanges.lock().unwrap().len();
        assert_eq!(
            remaining, 0,
            "fixture {} had {remaining} unconsumed exchange(s)",
            self.label
        );
    }
}

fn parse_response(v: &Value) -> HttpResponse {
    let mut headers = BTreeMap::new();
    if let Some(obj) = v["headers"].as_object() {
        for (k, val) in obj {
            headers.insert(
                k.to_ascii_lowercase(),
                val.as_str().unwrap_or_default().to_string(),
            );
        }
    }
    HttpResponse {
        status: v["status"].as_u64().unwrap_or(200) as u16,
        headers,
        body: v["body"].as_str().unwrap_or_default().to_string(),
    }
}

#[async_trait]
impl HttpTransport for ReplayTransport {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, DownloadError> {
        let ex = {
            let mut q = self.exchanges.lock().unwrap();
            q.pop_front().unwrap_or_else(|| {
                panic!(
                    "fixture {} exhausted but adapter sent another request: {} {}",
                    self.label, req.method, req.url
                )
            })
        };
        assert_request_matches(&self.label, &ex.expect, &req);
        Ok(ex.response)
    }
}

/// Assert the adapter's request satisfies the fixture's `expect` clause. Only
/// fields present in `expect` are asserted, so fixtures stay terse.
fn assert_request_matches(label: &str, expect: &Value, req: &HttpRequest) {
    if let Some(method) = expect["method"].as_str() {
        assert_eq!(
            req.method, method,
            "[{label}] method mismatch for {}",
            req.url
        );
    }
    for key in ["url_contains", "url_contains_2", "url_contains_3"] {
        if let Some(needle) = expect[key].as_str() {
            assert!(
                req.url.contains(needle),
                "[{label}] url {:?} does not contain {needle:?}",
                req.url
            );
        }
    }
    if let Some(needle) = expect["body_contains"].as_str() {
        let body = req.body.as_deref().unwrap_or("");
        // Compare ignoring whitespace so JSON spacing differences don't matter.
        let body_compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        let needle_compact: String = needle.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            body_compact.contains(&needle_compact),
            "[{label}] body {body:?} does not contain {needle:?}"
        );
    }
    if let Some(obj) = expect["header_equals"].as_object() {
        for (name, want) in obj {
            let got = req.headers.get(&name.to_ascii_lowercase());
            assert_eq!(
                got.map(String::as_str),
                want.as_str(),
                "[{label}] header {name} mismatch"
            );
        }
    }
}

/// Build a torrent grab request for a given download URL and category.
pub fn torrent_grab(download_url: &str, category: &str) -> GrabRequest {
    grab(download_url, category, Protocol::Torrent)
}

/// Build a Usenet grab request for a given download URL and category.
pub fn usenet_grab(download_url: &str, category: &str) -> GrabRequest {
    grab(download_url, category, Protocol::Usenet)
}

fn grab(download_url: &str, category: &str, protocol: Protocol) -> GrabRequest {
    let indexer_id = IndexerId::new();
    let content_ref = ContentRef::new(
        cellarr_core::ContentId::new(),
        LibraryId::new(),
        MediaType::Tv,
        Coordinates::Episode {
            season: 1,
            episode: 1,
            absolute: None,
        },
    )
    .expect("valid coords");
    GrabRequest {
        content_ref,
        release: Release {
            indexer_id,
            title: "Synthetic.Release.S01E01.1080p".into(),
            download_url: download_url.into(),
            guid: None,
            protocol,
            size: Some(1_000_000),
            seeders: Some(20),
            indexer_flags: vec![],
        },
        indexer_id,
        client_id: DownloadClientId::new(),
        category: category.into(),
    }
}
