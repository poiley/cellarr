//! `/api/v3` lookup contract: the shim must resolve **real** identities from the
//! metadata seam — a human title and the external id the ecosystem keys on
//! (`tvdbId`/`tmdbId`) — never the search term echoed back or a UUID (the Phase A
//! deferred gap), and must degrade gracefully when a source is unavailable.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use cellarr_api::{LookupCandidate, LookupOutcome, MetadataLookup, MetadataLookupError};
use cellarr_core::MediaType;
use serde_json::Value;

/// A mock metadata source: answers TV with a real Breaking Bad candidate and
/// reports movies as unavailable (the blocked-on-key path). This is the seam the
/// live `cellarr-meta` wiring also implements.
struct MockMetadata;

#[async_trait]
impl MetadataLookup for MockMetadata {
    async fn search(
        &self,
        media_type: MediaType,
        term: &str,
    ) -> Result<LookupOutcome, MetadataLookupError> {
        match media_type {
            MediaType::Tv => {
                // A source returns its real catalogue regardless of the exact
                // term; we key off a substring to keep the mock simple but still
                // prove the title is NOT just the echoed term.
                if term.to_lowercase().contains("breaking") {
                    Ok(LookupOutcome::Resolved(vec![LookupCandidate {
                        source_id: "81189".to_string(),
                        media_type: MediaType::Tv,
                        title: "Breaking Bad".to_string(),
                        year: Some(2008),
                        overview: Some("A chemistry teacher turns to meth.".to_string()),
                        external_ids: vec![
                            ("tvdb".to_string(), "81189".to_string()),
                            ("imdb".to_string(), "tt0903747".to_string()),
                        ],
                    }]))
                } else {
                    Ok(LookupOutcome::Resolved(vec![]))
                }
            }
            MediaType::Movie => Ok(LookupOutcome::Unavailable(
                "no TMDb API key configured (set CELLARR_TMDB__API_KEY)".to_string(),
            )),
            other => Ok(LookupOutcome::Unavailable(format!(
                "no source for {other:?}"
            ))),
        }
    }
}

#[tokio::test]
async fn series_lookup_resolves_real_tvdb_id_and_title() {
    let server = common::start_with_metadata(Arc::new(MockMetadata)).await;
    let client = server.client();

    // The Sonarr face's series lookup must resolve to the real series.
    let resp = client
        .get(server.url("/sonarr/api/v3/series/lookup?term=Breaking%20Bad"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 200, "lookup should succeed");
    let body: Vec<Value> = resp.json().await.expect("json array");
    assert!(!body.is_empty(), "expected at least one candidate");

    let first = &body[0];
    // The acceptance criteria: correct tvdbId (81189) and a human title.
    assert_eq!(
        first.get("tvdbId").and_then(Value::as_i64),
        Some(81189),
        "candidate must carry the real tvdbId, got {first}"
    );
    assert_eq!(
        first.get("title").and_then(Value::as_str),
        Some("Breaking Bad"),
        "candidate title must be the real series name, not the term/UUID"
    );
    assert_eq!(first.get("year").and_then(Value::as_i64), Some(2008));
    assert_eq!(
        first.get("titleSlug").and_then(Value::as_str),
        Some("breaking-bad")
    );
    // imdbId cross-reference is surfaced too.
    assert_eq!(
        first.get("imdbId").and_then(Value::as_str),
        Some("tt0903747")
    );
}

#[tokio::test]
async fn series_lookup_title_is_not_the_echoed_term() {
    // Guard against the fake-green pattern where lookup echoes the search term as
    // the title. We search with a deliberately wrong-cased / extra term and assert
    // the resolved title is the canonical one, not what we typed.
    let server = common::start_with_metadata(Arc::new(MockMetadata)).await;
    let client = server.client();
    let resp = client
        .get(server.url("/sonarr/api/v3/series/lookup?term=breaking+bad+1080p"))
        .send()
        .await
        .expect("request");
    let body: Vec<Value> = resp.json().await.expect("json");
    assert_eq!(
        body[0].get("title").and_then(Value::as_str),
        Some("Breaking Bad"),
        "title must be resolved, never the echoed term"
    );
}

#[tokio::test]
async fn movie_lookup_degrades_gracefully_when_unavailable() {
    // No TMDb key → the Radarr face's movie lookup returns an empty array (200),
    // never a 500 — offline non-negotiable.
    let server = common::start_with_metadata(Arc::new(MockMetadata)).await;
    let client = server.client();
    let resp = client
        .get(server.url("/radarr/api/v3/movie/lookup?term=The%20Matrix"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 200, "unavailable source must not error");
    let body: Vec<Value> = resp.json().await.expect("json array");
    assert!(body.is_empty(), "unavailable lookup yields an empty result");
}

#[tokio::test]
async fn lookup_with_no_metadata_source_is_empty_not_error() {
    // A server with no metadata wiring at all (offline default) returns an empty
    // lookup, never a 500.
    let server = common::start_open().await;
    let client = server.client();
    let resp = client
        .get(server.url("/sonarr/api/v3/series/lookup?term=Breaking%20Bad"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 200);
    let body: Vec<Value> = resp.json().await.expect("json array");
    assert!(body.is_empty());
}
