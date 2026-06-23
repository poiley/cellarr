//! The provider abstraction and the structured-output contract.
//!
//! A [`Provider`] is whatever can answer an inference query: the local model is
//! the primary implementation, an optional remote one is an enhancement. Every
//! provider returns **structured output only** — a schema-constrained
//! [`InferredParse`] / [`InferredMatch`], never free-form text. The orchestrator
//! ([`crate::Fallback`]) validates the structure before trusting it, so a
//! hallucinated or malformed response is rejected rather than acted on.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use cellarr_core::{Confidence, Coordinates, ParsedRelease, Resolution, Source, VideoCodec};

use crate::error::LlmError;

/// The kind of query a provider is asked to answer.
///
/// Kept as a small closed enum so a provider can specialise its prompt/schema
/// per task while the orchestrator stays task-agnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    /// "Parse this release title into structured fields."
    Parse {
        /// The release title the deterministic parser was unsure about.
        title: String,
        /// Optional surrounding context (e.g. the indexer category, sibling
        /// titles) the model may use as a hint.
        context: Option<String>,
    },
    /// "Which of these candidates does this parse describe?"
    Match {
        /// The (low-confidence) parse to disambiguate.
        title: String,
        /// Human-readable candidate descriptions, indexed by position. The model
        /// answers with the index it believes is correct.
        candidates: Vec<String>,
    },
}

impl Query {
    /// The normalized cache key for this query.
    ///
    /// Caching is by *normalized input* so any given hard title costs at most one
    /// inference ever (the spec's "weird title costs at most one inference"
    /// guarantee). Normalization lower-cases and collapses whitespace so trivial
    /// formatting differences share a cache entry.
    #[must_use]
    pub fn cache_key(&self) -> String {
        fn norm(s: &str) -> String {
            s.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        }
        match self {
            Query::Parse { title, context } => {
                format!(
                    "parse:{}|{}",
                    norm(title),
                    norm(context.as_deref().unwrap_or(""))
                )
            }
            Query::Match { title, candidates } => {
                let cands = candidates
                    .iter()
                    .map(|c| norm(c))
                    .collect::<Vec<_>>()
                    .join("\u{1f}");
                format!("match:{}|{}", norm(title), cands)
            }
        }
    }
}

/// A schema-constrained parse as returned by an inference provider.
///
/// This is deliberately a *subset* of the fields [`ParsedRelease`] carries: the
/// fields a language model can plausibly infer from a title. It is converted to a
/// [`ParsedRelease`] (with an inference-derived confidence) by
/// [`InferredParse::into_parsed`]. Unknown fields in the wire payload are
/// rejected (`deny_unknown_fields`) so a model that invents keys is caught.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferredParse {
    /// The cleaned title the model believes this release is for.
    pub clean_title: String,
    /// Inferred resolution, when the model committed to one.
    #[serde(default)]
    pub resolution: Option<Resolution>,
    /// Inferred source/medium.
    #[serde(default)]
    pub source: Option<Source>,
    /// Inferred video codec.
    #[serde(default)]
    pub codec: Option<VideoCodec>,
    /// Inferred year.
    #[serde(default)]
    pub year: Option<u16>,
    /// Inferred numbering, when applicable.
    #[serde(default)]
    pub coordinates: Vec<Coordinates>,
    /// The model's self-reported confidence in `0.0..=1.0`. Clamped on
    /// conversion; an out-of-range value never escapes as a nonsensical score.
    pub confidence: f32,
}

impl InferredParse {
    /// Validate the structured output, rejecting anything that cannot be trusted.
    ///
    /// # Errors
    /// Returns [`LlmError::InvalidOutput`] when the clean title is empty or the
    /// confidence is not a finite number — a model that returns `NaN` or nothing
    /// is treated as having said nothing.
    pub fn validate(&self) -> Result<(), LlmError> {
        if self.clean_title.trim().is_empty() {
            return Err(LlmError::InvalidOutput("empty clean_title".into()));
        }
        if !self.confidence.is_finite() {
            return Err(LlmError::InvalidOutput("non-finite confidence".into()));
        }
        Ok(())
    }

    /// Convert into a [`ParsedRelease`] tagged with the inference confidence.
    ///
    /// The original `raw_title` is preserved (the model never gets to rewrite the
    /// provenance the corpus and decision log record). The confidence is recorded
    /// against every field the model populated so downstream gating can see this
    /// parse came from inference.
    #[must_use]
    pub fn into_parsed(self, raw_title: impl Into<String>) -> ParsedRelease {
        use cellarr_core::ParsedField;

        let mut p = ParsedRelease::new(raw_title);
        let conf = Confidence::new(self.confidence);
        p.clean_title = Some(self.clean_title);
        if let Some(r) = self.resolution {
            p.resolution = Some(r);
            p.set_confidence(ParsedField::Resolution, conf);
        }
        if let Some(s) = self.source {
            p.source = Some(s);
            p.set_confidence(ParsedField::Source, conf);
        }
        if let Some(c) = self.codec {
            p.codec = Some(c);
            p.set_confidence(ParsedField::Codec, conf);
        }
        if let Some(y) = self.year {
            p.year = Some(y);
            p.set_confidence(ParsedField::Year, conf);
        }
        if !self.coordinates.is_empty() {
            p.coordinates = self.coordinates;
            p.set_confidence(ParsedField::Coordinates, conf);
        }
        p
    }
}

/// A schema-constrained disambiguation answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferredMatch {
    /// The index into the candidate list the model selected.
    pub candidate_index: usize,
    /// The model's self-reported confidence in `0.0..=1.0`.
    pub confidence: f32,
}

impl InferredMatch {
    /// Validate the answer against the candidate count.
    ///
    /// # Errors
    /// Returns [`LlmError::InvalidOutput`] if the selected index is out of range
    /// (a model pointing at a candidate that does not exist) or the confidence is
    /// not finite.
    pub fn validate(&self, candidate_count: usize) -> Result<(), LlmError> {
        if self.candidate_index >= candidate_count {
            return Err(LlmError::InvalidOutput(format!(
                "candidate_index {} out of range for {} candidates",
                self.candidate_index, candidate_count
            )));
        }
        if !self.confidence.is_finite() {
            return Err(LlmError::InvalidOutput("non-finite confidence".into()));
        }
        Ok(())
    }
}

/// The structured answer a provider returns for a [`Query`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    /// The answer to a [`Query::Parse`].
    Parse(InferredParse),
    /// The answer to a [`Query::Match`].
    Match(InferredMatch),
}

/// Anything that can answer an inference [`Query`] with structured output.
///
/// Implemented by the in-crate [`crate::FixtureProvider`] (always available) and,
/// behind features, by the local and cloud providers. Object-safe so the daemon
/// can hold the configured provider behind `dyn`.
#[async_trait]
pub trait Provider: Send + Sync {
    /// A human-facing name for logs.
    fn name(&self) -> &str;

    /// Answer a query with schema-constrained structured output.
    ///
    /// # Errors
    /// Returns [`LlmError`] when the provider is unavailable, the request fails,
    /// or the model returns output that fails schema validation.
    async fn infer(&self, query: &Query) -> Result<Response, LlmError>;
}
