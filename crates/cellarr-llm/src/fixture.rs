//! A network-free provider backed by recorded responses.
//!
//! [`FixtureProvider`] is the record/replay seam the spec's cached-fixture tests
//! use: it answers a [`Query`] from a table of pre-recorded [`Response`]s keyed by
//! the query's normalized cache key, with **no network and no model**. It is
//! always compiled (no feature gate) so the trait shape and the orchestrator's
//! caching/gating logic can be exercised entirely offline.
//!
//! The fixtures are synthetic — hand-authored examples of the shape a real model
//! would return — not captures of any live service.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use crate::error::LlmError;
use crate::provider::{Provider, Query, Response};

/// A provider that replays recorded structured responses.
pub struct FixtureProvider {
    name: String,
    responses: HashMap<String, Response>,
    /// Counts how many times the provider was actually invoked. The cache tests
    /// assert this stays at 1 across a repeated query (cache hit avoids a second
    /// inference).
    calls: AtomicUsize,
}

impl FixtureProvider {
    /// Build a provider from `(query, response)` fixtures.
    ///
    /// Each fixture is keyed by its [`Query::cache_key`] so lookup matches the
    /// orchestrator's own normalization.
    #[must_use]
    pub fn new(fixtures: impl IntoIterator<Item = (Query, Response)>) -> Self {
        let responses = fixtures
            .into_iter()
            .map(|(q, r)| (q.cache_key(), r))
            .collect();
        Self {
            name: "fixture".to_string(),
            responses,
            calls: AtomicUsize::new(0),
        }
    }

    /// How many times [`Provider::infer`] has been invoked. Used by tests to
    /// prove the cache short-circuits repeat queries.
    #[must_use]
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Provider for FixtureProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn infer(&self, query: &Query) -> Result<Response, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.responses
            .get(&query.cache_key())
            .cloned()
            .ok_or_else(|| LlmError::Unavailable(format!("no fixture for {}", query.cache_key())))
    }
}
