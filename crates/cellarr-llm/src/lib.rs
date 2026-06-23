//! cellarr-llm — the optional, local-first inference fallback.
//!
//! When `cellarr-parse`'s deterministic parse is below a confidence threshold, or
//! when Identify cannot pick between candidates, the pipeline *may* consult this
//! crate for a structured suggestion. Everything here is **optional and
//! offline-first**:
//!
//! - **No provider is compiled by default.** The local provider lives behind the
//!   `local` feature, the optional remote one behind `cloud`. The default build
//!   links neither, so the single-binary, offline non-negotiable holds and the
//!   daemon is fully functional with inference disabled. The in-crate
//!   [`FixtureProvider`] is always available so the trait shape and the
//!   orchestrator's caching/gating logic can be exercised with no network.
//! - **Structured output only.** Providers return schema-constrained
//!   [`InferredParse`] / [`InferredMatch`] values, validated before use; free-form
//!   text and malformed output are rejected, never trusted.
//! - **Cached by normalized input.** Results are memoized on the query's
//!   [`Query::cache_key`], so any given hard title costs at most one inference
//!   ever.
//!
//! # Never authoritative for destructive operations
//!
//! An inference-derived result may *inform* a grab suggestion, but it must never
//! drive a destructive Import (one that would replace or delete an existing file)
//! on its own. [`Fallback::match_fallback`] returns a [`GatedMatch`] whose
//! [`GatedMatch::allows_destructive`] is true only when the match clears the
//! configured destructive-confidence gate. A low-confidence, inference-derived
//! match is *held* for user confirmation — a hallucinated parse must never
//! overwrite the wrong file (see `docs/03-pipeline.md`). The caller is
//! responsible for honouring the gate; this crate makes the gate explicit in the
//! return type so it cannot be silently ignored.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod error;
mod fixture;
mod provider;

#[cfg(feature = "cloud")]
mod cloud;
#[cfg(feature = "local")]
mod local;

use std::sync::Arc;

use moka::future::Cache;

use cellarr_core::{Confidence, ContentMatch, ContentRef, ParsedRelease};

pub use error::LlmError;
pub use fixture::FixtureProvider;
pub use provider::{InferredMatch, InferredParse, Provider, Query, Response};

#[cfg(feature = "cloud")]
pub use cloud::CloudProvider;
#[cfg(feature = "local")]
pub use local::LocalProvider;

/// Confidence thresholds that gate when (and how authoritatively) a fallback
/// result is used.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateConfig {
    /// Below this aggregate parse confidence, the deterministic parse is weak
    /// enough that consulting inference is worthwhile. (The caller decides whether
    /// to call; this is the documented intent.)
    pub consult_below: f32,
    /// At or above this confidence, an inference-derived match may drive a
    /// **destructive** Import. Below it, the match is held for user confirmation.
    /// Defaults high: inference is never casually allowed to overwrite a file.
    pub destructive_at_or_above: f32,
}

impl Default for GateConfig {
    fn default() -> Self {
        // Consult inference when the deterministic parse is shaky; never let a
        // sub-0.95 inference result overwrite an existing file unattended.
        Self {
            consult_below: 0.5,
            destructive_at_or_above: 0.95,
        }
    }
}

/// A match suggestion paired with the gate decision for destructive use.
///
/// The gate is part of the return type precisely so a caller cannot use an
/// inference-derived match for a file-replacing Import without first reading
/// [`allows_destructive`](GatedMatch::allows_destructive).
#[derive(Debug, Clone, PartialEq)]
pub struct GatedMatch {
    /// The suggested match.
    pub content_match: ContentMatch,
    /// Whether this match cleared the destructive-confidence gate. When `false`,
    /// the match may inform a non-destructive suggestion but a destructive Import
    /// must be **held for user confirmation**.
    pub allows_destructive: bool,
}

/// The inference-fallback orchestrator.
///
/// Holds the configured [`Provider`] behind `Arc<dyn>` (so the daemon can pick
/// one at runtime) and a [`moka`] cache keyed by normalized input. Both
/// [`parse_fallback`](Fallback::parse_fallback) and
/// [`match_fallback`](Fallback::match_fallback) consult the cache before the
/// provider, guaranteeing at most one inference per distinct hard input.
pub struct Fallback {
    provider: Arc<dyn Provider>,
    gate: GateConfig,
    cache: Cache<String, Response>,
}

impl Fallback {
    /// Build a fallback over `provider` with the default gate and a bounded cache.
    #[must_use]
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self::with_config(provider, GateConfig::default())
    }

    /// Build a fallback with an explicit [`GateConfig`].
    #[must_use]
    pub fn with_config(provider: Arc<dyn Provider>, gate: GateConfig) -> Self {
        Self {
            provider,
            gate,
            cache: Cache::new(10_000),
        }
    }

    /// The configured gate.
    #[must_use]
    pub fn gate(&self) -> GateConfig {
        self.gate
    }

    /// Run `query` through the cache, then the provider on a miss.
    ///
    /// On a cache miss the provider is consulted and a *successful* response is
    /// stored under the query's normalized key. Errors are not cached: a transient
    /// provider failure must not poison the entry for a title that might succeed
    /// later.
    async fn cached(&self, query: &Query) -> Result<Response, LlmError> {
        let key = query.cache_key();
        if let Some(hit) = self.cache.get(&key).await {
            return Ok(hit);
        }
        let resp = self.provider.infer(query).await?;
        self.cache.insert(key, resp.clone()).await;
        Ok(resp)
    }

    /// Structured-output parse fallback for a low-confidence title.
    ///
    /// Returns `None` rather than an error when the provider is unavailable or
    /// returns nothing usable — inference is a hint, so a failure degrades to "no
    /// suggestion", never to a pipeline failure. A validated parse is converted to
    /// a [`ParsedRelease`] tagged with the inference confidence; the original
    /// `title` is preserved as the provenance `raw_title`.
    pub async fn parse_fallback(
        &self,
        title: &str,
        context: Option<&str>,
    ) -> Option<ParsedRelease> {
        let query = Query::Parse {
            title: title.to_string(),
            context: context.map(str::to_string),
        };
        match self.cached(&query).await {
            Ok(Response::Parse(inferred)) => match inferred.validate() {
                Ok(()) => Some(inferred.into_parsed(title)),
                Err(_) => None,
            },
            // A match response to a parse query is a schema violation; ignore it.
            Ok(Response::Match(_)) | Err(_) => None,
        }
    }

    /// Structured-output disambiguation fallback.
    ///
    /// Given a parse and the candidate content nodes it might satisfy (each with a
    /// human-readable description for the model), asks the provider to pick one and
    /// returns the selected [`ContentMatch`] wrapped in a [`GatedMatch`] that
    /// records whether the result is confident enough for a destructive Import.
    ///
    /// `candidates` pairs each [`ContentRef`] with the description shown to the
    /// model; the model answers with an index into that list. Returns `None` when
    /// inference is unavailable, the answer fails schema validation, or the index
    /// is out of range.
    pub async fn match_fallback(
        &self,
        parsed: &ParsedRelease,
        candidates: &[(ContentRef, String)],
    ) -> Option<GatedMatch> {
        if candidates.is_empty() {
            return None;
        }
        let query = Query::Match {
            title: parsed.raw_title.clone(),
            candidates: candidates.iter().map(|(_, desc)| desc.clone()).collect(),
        };
        let inferred = match self.cached(&query).await {
            Ok(Response::Match(m)) => m,
            // A parse response to a match query is a schema violation; ignore it.
            Ok(Response::Parse(_)) | Err(_) => return None,
        };
        inferred.validate(candidates.len()).ok()?;

        let (content_ref, _) = candidates.get(inferred.candidate_index)?;
        let confidence = Confidence::new(inferred.confidence);
        let allows_destructive = confidence.value() >= self.gate.destructive_at_or_above;
        Some(GatedMatch {
            content_match: ContentMatch {
                content_ref: content_ref.clone(),
                confidence,
            },
            allows_destructive,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::{ContentId, Coordinates, LibraryId, MediaType, Resolution, Source};

    fn content_ref(lib: LibraryId, season: u32, episode: u32) -> ContentRef {
        ContentRef::new(
            ContentId::new(),
            lib,
            MediaType::Tv,
            Coordinates::Episode {
                season,
                episode,
                absolute: None,
            },
        )
        .expect("tv coords")
    }

    fn parse_fixture(title: &str) -> (Query, Response) {
        (
            Query::Parse {
                title: title.to_string(),
                context: None,
            },
            Response::Parse(InferredParse {
                clean_title: "The Show".into(),
                resolution: Some(Resolution::R1080p),
                source: Some(Source::WebDl),
                codec: None,
                year: Some(2019),
                coordinates: vec![Coordinates::Episode {
                    season: 1,
                    episode: 2,
                    absolute: None,
                }],
                confidence: 0.8,
            }),
        )
    }

    // The trait shape is exercisable with no feature and no network.
    #[tokio::test]
    async fn fixture_provider_answers_a_parse_query() {
        let title = "The.Show.S01E02.WEB-DL.1080p";
        let provider = FixtureProvider::new([parse_fixture(title)]);
        let resp = provider
            .infer(&Query::Parse {
                title: title.to_string(),
                context: None,
            })
            .await
            .expect("fixture present");
        assert!(matches!(resp, Response::Parse(_)));
    }

    #[tokio::test]
    async fn parse_fallback_returns_structured_parse_then_hits_cache() {
        let title = "The.Show.S01E02.WEB-DL.1080p";
        let provider = Arc::new(FixtureProvider::new([parse_fixture(title)]));
        let fallback = Fallback::new(provider.clone());

        let first = fallback
            .parse_fallback(title, None)
            .await
            .expect("structured parse");
        assert_eq!(first.raw_title, title);
        assert_eq!(first.clean_title.as_deref(), Some("The Show"));
        assert_eq!(first.resolution, Some(Resolution::R1080p));
        assert_eq!(first.year, Some(2019));
        assert_eq!(provider.call_count(), 1);

        // A second, normalized-equivalent call must hit the cache: the provider is
        // not invoked again.
        let second = fallback
            .parse_fallback("the.show.s01e02.web-dl.1080p   ", None)
            .await
            .expect("cached parse");
        assert_eq!(second.clean_title.as_deref(), Some("The Show"));
        assert_eq!(
            provider.call_count(),
            1,
            "cache hit must avoid a second inference"
        );
    }

    #[tokio::test]
    async fn malformed_output_is_rejected_not_trusted() {
        let title = "garbage";
        // Empty clean_title violates the schema invariant.
        let bad = Response::Parse(InferredParse {
            clean_title: "   ".into(),
            resolution: None,
            source: None,
            codec: None,
            year: None,
            coordinates: vec![],
            confidence: 0.9,
        });
        let provider = Arc::new(FixtureProvider::new([(
            Query::Parse {
                title: title.to_string(),
                context: None,
            },
            bad,
        )]));
        let fallback = Fallback::new(provider);
        assert!(fallback.parse_fallback(title, None).await.is_none());
    }

    #[tokio::test]
    async fn unavailable_provider_degrades_to_none() {
        // No fixtures registered → provider returns Unavailable → None, not panic.
        let provider = Arc::new(FixtureProvider::new(std::iter::empty()));
        let fallback = Fallback::new(provider);
        assert!(fallback.parse_fallback("anything", None).await.is_none());
    }

    #[tokio::test]
    async fn low_confidence_match_is_held_from_destructive() {
        let title = "Ambiguous.Release.2019";
        let provider = Arc::new(FixtureProvider::new([(
            Query::Match {
                title: title.to_string(),
                candidates: vec!["S01E02 of The Show".into(), "S02E01 of The Show".into()],
            },
            Response::Match(InferredMatch {
                candidate_index: 0,
                confidence: 0.6, // below the destructive gate
            }),
        )]));
        let fallback = Fallback::new(provider);

        let library = LibraryId::new();
        let parsed = ParsedRelease::new(title);
        let candidates = vec![
            (content_ref(library, 1, 2), "S01E02 of The Show".to_string()),
            (content_ref(library, 2, 1), "S02E01 of The Show".to_string()),
        ];
        let gated = fallback
            .match_fallback(&parsed, &candidates)
            .await
            .expect("match suggestion");
        assert_eq!(gated.content_match.confidence.value(), 0.6);
        assert!(
            !gated.allows_destructive,
            "a sub-gate inference match must be held for confirmation, not allowed to overwrite"
        );
    }

    #[tokio::test]
    async fn high_confidence_match_clears_destructive_gate() {
        let title = "Clear.Release.2019";
        let provider = Arc::new(FixtureProvider::new([(
            Query::Match {
                title: title.to_string(),
                candidates: vec!["S01E02 of The Show".into()],
            },
            Response::Match(InferredMatch {
                candidate_index: 0,
                confidence: 0.99,
            }),
        )]));
        let fallback = Fallback::new(provider);

        let library = LibraryId::new();
        let parsed = ParsedRelease::new(title);
        let candidates = vec![(content_ref(library, 1, 2), "S01E02 of The Show".to_string())];
        let gated = fallback
            .match_fallback(&parsed, &candidates)
            .await
            .expect("match suggestion");
        assert!(gated.allows_destructive);
    }

    #[tokio::test]
    async fn out_of_range_match_index_is_rejected() {
        let title = "Bad.Index.2019";
        let provider = Arc::new(FixtureProvider::new([(
            Query::Match {
                title: title.to_string(),
                candidates: vec!["only one candidate".into()],
            },
            Response::Match(InferredMatch {
                candidate_index: 5,
                confidence: 0.99,
            }),
        )]));
        let fallback = Fallback::new(provider);
        let parsed = ParsedRelease::new(title);
        let candidates = vec![(
            content_ref(LibraryId::new(), 1, 1),
            "only one candidate".to_string(),
        )];
        assert!(fallback
            .match_fallback(&parsed, &candidates)
            .await
            .is_none());
    }
}
