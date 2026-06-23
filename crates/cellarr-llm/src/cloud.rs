//! The optional remote provider (behind the `cloud` feature).
//!
//! An *enhancement only*: the spec is explicit that a cloud provider is never a
//! requirement and the offline non-negotiable always holds. It is structurally
//! identical to the local provider — schema-constrained JSON in, validated
//! structured output out — but points at a remote endpoint and carries an API
//! key. It is compiled only when the `cloud` feature is on, so the default,
//! offline build never links an outbound HTTP client for inference.

use async_trait::async_trait;

use crate::error::LlmError;
use crate::provider::{InferredMatch, InferredParse, Provider, Query, Response};

/// A provider backed by a remote inference API.
pub struct CloudProvider {
    name: String,
    endpoint: String,
    api_key: String,
    client: reqwest::Client,
}

impl CloudProvider {
    /// Construct a provider pointed at a remote `endpoint` with an `api_key`.
    ///
    /// # Errors
    /// Returns [`LlmError::Unavailable`] if the HTTP client cannot be built.
    pub fn new(endpoint: impl Into<String>, api_key: impl Into<String>) -> Result<Self, LlmError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| LlmError::Unavailable(e.to_string()))?;
        Ok(Self {
            name: "cloud".to_string(),
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            client,
        })
    }
}

#[async_trait]
impl Provider for CloudProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn infer(&self, query: &Query) -> Result<Response, LlmError> {
        let prompt = match query {
            Query::Parse { title, context } => {
                format!("parse {title:?} ctx {:?}", context.as_deref().unwrap_or(""))
            }
            Query::Match { title, candidates } => format!("match {title:?} of {candidates:?}"),
        };
        let body = serde_json::json!({ "prompt": prompt, "response_format": "json" });
        let resp = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Unavailable(e.to_string()))?;
        let text = resp
            .text()
            .await
            .map_err(|e| LlmError::Request(e.to_string()))?;

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
