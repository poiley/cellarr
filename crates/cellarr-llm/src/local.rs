//! The local-first provider (behind the `local` feature).
//!
//! Talks to a model server reachable over HTTP on the local machine (e.g.
//! llama.cpp's server or Ollama). This is the *primary* provider: the spec's
//! offline non-negotiable means inference, when enabled at all, must work against
//! a local model with no cloud dependency.
//!
//! The wire request asks the server for schema-constrained JSON; the response is
//! deserialized into the crate's structured-output types and validated by the
//! orchestrator before it is trusted. No network call is made unless this feature
//! is compiled in *and* a provider is configured.

use async_trait::async_trait;

use crate::error::LlmError;
use crate::provider::{InferredMatch, InferredParse, Provider, Query, Response};

/// A provider backed by a local HTTP model server.
pub struct LocalProvider {
    name: String,
    endpoint: String,
    client: reqwest::Client,
}

impl LocalProvider {
    /// Construct a provider pointed at a local model `endpoint`
    /// (e.g. `http://127.0.0.1:11434/v1/chat/completions`).
    ///
    /// # Errors
    /// Returns [`LlmError::Unavailable`] if the HTTP client cannot be built.
    pub fn new(endpoint: impl Into<String>) -> Result<Self, LlmError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| LlmError::Unavailable(e.to_string()))?;
        Ok(Self {
            name: "local".to_string(),
            endpoint: endpoint.into(),
            client,
        })
    }

    /// The prompt this query maps to. Kept here so the schema the server is asked
    /// to honour lives next to the types it deserializes into.
    fn prompt(query: &Query) -> String {
        match query {
            Query::Parse { title, context } => format!(
                "Parse this release title into JSON with fields \
                 {{clean_title, resolution, source, codec, year, coordinates, confidence}}. \
                 Title: {title}. Context: {}",
                context.as_deref().unwrap_or("(none)")
            ),
            Query::Match { title, candidates } => format!(
                "Given title {title:?}, return JSON {{candidate_index, confidence}} \
                 selecting the best of: {candidates:?}"
            ),
        }
    }
}

#[async_trait]
impl Provider for LocalProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn infer(&self, query: &Query) -> Result<Response, LlmError> {
        let body = serde_json::json!({
            "prompt": Self::prompt(query),
            "format": "json",
        });
        let resp = self
            .client
            .post(&self.endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Unavailable(e.to_string()))?;
        let text = resp
            .text()
            .await
            .map_err(|e| LlmError::Request(e.to_string()))?;

        // The server returns schema-constrained JSON; deserialize into the
        // structured-output type for the query kind. Free-form text that does not
        // deserialize is rejected as invalid output, never trusted.
        match query {
            Query::Parse { .. } => {
                let parsed: InferredParse = serde_json::from_str(&text)
                    .map_err(|e| LlmError::InvalidOutput(e.to_string()))?;
                Ok(Response::Parse(parsed))
            }
            Query::Match { .. } => {
                let m: InferredMatch = serde_json::from_str(&text)
                    .map_err(|e| LlmError::InvalidOutput(e.to_string()))?;
                Ok(Response::Match(m))
            }
        }
    }
}
