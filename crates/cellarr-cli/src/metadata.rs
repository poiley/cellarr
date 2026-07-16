//! Wiring the live `cellarr-meta` sources into the API's metadata seam and the
//! media identify path.
//!
//! This is the one place the concrete metadata sources (TheTVDB for TV, TMDb for
//! movies) are bound to the abstract seams the rest of cellarr depends on:
//!
//! - [`cellarr_api::MetadataLookup`] — the shim's `series/lookup`/`movie/lookup`
//!   resolve real titles + external ids through this (closing the Phase A
//!   UUID-title gap).
//! - [`cellarr_media::SceneMappingProvider`] — Identify's anime absolute→episode
//!   remap reads TheXEM scene mappings (keyed by TVDB id) through this.
//!
//! Both degrade gracefully with no key: the API lookup reports the media type as
//! [`LookupOutcome::Unavailable`] (an empty, flagged result — never a 500), and
//! the scene-mapping provider reports "no mapping" so Identify surfaces an
//! absolute number for manual resolution rather than crashing the daemon. Offline
//! is non-negotiable.

use async_trait::async_trait;

use cellarr_api::{LookupCandidate, LookupOutcome, MetadataLookup, MetadataLookupError};
use cellarr_core::MediaType;
use cellarr_media::{SceneMapping, SceneMappingProvider, SceneRange};
use cellarr_meta::{Fetcher, MetaError, ReqwestFetcher, SearchResult, TheTvdbSource, TmdbSource};

use crate::config::Config;

/// The live metadata wiring: the concrete TheTVDB (TV) and TMDb (movie) sources
/// behind the API's [`MetadataLookup`] seam.
///
/// Generic over the [`Fetcher`] so the daemon binds the real `reqwest` transport
/// ([`LiveMetadata::from_config`]) while tests bind a recorded one — the same
/// degradation/normalization logic runs in both. Holds both sources so a single
/// instance answers TV and movie lookups; each reports unavailable independently
/// when its key is absent.
pub struct LiveMetadata<F: Fetcher = ReqwestFetcher> {
    tvdb: TheTvdbSource<F>,
    tmdb: TmdbSource<F>,
    /// Whether a TheTVDB key is configured (the offline-degradation gate, read
    /// without exposing the key).
    tvdb_configured: bool,
    /// Whether a TMDb key is configured.
    tmdb_configured: bool,
}

impl LiveMetadata<ReqwestFetcher> {
    /// Build the live metadata sources from the loaded daemon config.
    ///
    /// Keys come from `CELLARR_TVDB__*` / `CELLARR_TMDB__*` (the gitignored
    /// `.env`); absent keys leave the corresponding source unavailable. The key
    /// values are never logged.
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let tvdb_configured = config.tvdb.api_key.is_some();
        let tmdb_configured = config.tmdb.api_key.is_some();
        let tvdb = TheTvdbSource::new(
            ReqwestFetcher::new("thetvdb"),
            config.thetvdb_source_config(),
        );
        let tmdb = TmdbSource::new(ReqwestFetcher::new("tmdb"), config.tmdb_source_config());
        Self {
            tvdb,
            tmdb,
            tvdb_configured,
            tmdb_configured,
        }
    }
}

impl<F: Fetcher> LiveMetadata<F> {
    /// Construct directly from already-built sources (used by tests over a
    /// recorded fetcher). `tvdb_configured`/`tmdb_configured` are the offline
    /// gates the live constructor derives from the presence of each key.
    #[must_use]
    pub fn from_sources(
        tvdb: TheTvdbSource<F>,
        tmdb: TmdbSource<F>,
        tvdb_configured: bool,
        tmdb_configured: bool,
    ) -> Self {
        Self {
            tvdb,
            tmdb,
            tvdb_configured,
            tmdb_configured,
        }
    }
}

/// Map a [`MetaError`] from a configured source into the API's lookup outcome /
/// error. A missing credential degrades to [`LookupOutcome::Unavailable`] (never
/// an error); everything else is a genuine upstream failure the shim renders as a
/// 502-style structured error.
fn classify(err: MetaError, provider: &str) -> Result<LookupOutcome, MetadataLookupError> {
    match err {
        MetaError::NoCredential { src } => Ok(LookupOutcome::Unavailable(format!(
            "metadata source '{src}' has no API key configured"
        ))),
        other => Err(MetadataLookupError {
            provider: provider.to_string(),
            detail: other.to_string(),
        }),
    }
}

/// Convert a normalized `cellarr-meta` [`SearchResult`] into the API's
/// [`LookupCandidate`]. The source id is carried into the external-id list under
/// its native scheme (`tvdb`/`tmdb`) so the shim always has the id the ecosystem
/// keys on even when the search payload carried no cross-references.
fn to_candidate(r: SearchResult) -> LookupCandidate {
    let scheme = match r.media_type {
        MediaType::Tv => "tvdb",
        _ => "tmdb",
    };
    let mut external_ids = r.external_ids;
    if !external_ids
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case(scheme))
    {
        external_ids.push((scheme.to_string(), r.source_id.clone()));
    }
    LookupCandidate {
        source_id: r.source_id,
        media_type: r.media_type,
        title: r.title,
        year: r.year,
        overview: r.overview,
        external_ids,
        prominence: r.prominence,
        alt_titles: r.alt_titles,
    }
}

#[async_trait]
impl<F: Fetcher> MetadataLookup for LiveMetadata<F> {
    async fn search(
        &self,
        media_type: MediaType,
        term: &str,
    ) -> Result<LookupOutcome, MetadataLookupError> {
        match media_type {
            MediaType::Tv => {
                if !self.tvdb_configured {
                    return Ok(LookupOutcome::Unavailable(
                        "no TheTVDB API key configured (set CELLARR_TVDB__API_KEY)".to_string(),
                    ));
                }
                match self.tvdb.search_normalized(term).await {
                    Ok(results) => Ok(LookupOutcome::Resolved(
                        results.into_iter().map(to_candidate).collect(),
                    )),
                    Err(e) => classify(e, "thetvdb"),
                }
            }
            MediaType::Movie => {
                if !self.tmdb_configured {
                    // TMDb is blocked-on-key in this deployment: degrade clearly
                    // (the daemon stays up) rather than erroring.
                    return Ok(LookupOutcome::Unavailable(
                        "no TMDb API key configured (set CELLARR_TMDB__API_KEY)".to_string(),
                    ));
                }
                match self.tmdb.search_normalized(term).await {
                    Ok(results) => Ok(LookupOutcome::Resolved(
                        results.into_iter().map(to_candidate).collect(),
                    )),
                    Err(e) => classify(e, "tmdb"),
                }
            }
            // No music/book metadata sources in v1; report unavailable.
            other => Ok(LookupOutcome::Unavailable(format!(
                "no metadata source for {other:?}"
            ))),
        }
    }
}

/// The Identify-side scene-mapping provider, backed by TheTVDB's TheXEM map.
///
/// Identify remaps an anime release's absolute episode number to a canonical
/// season/episode through [`cellarr_media::remap_absolute`], which reads mappings
/// via this seam keyed by the series' TVDB id. We fetch the series' TheXEM scene
/// map live (cached) and distill its rules into the media crate's [`SceneMapping`]
/// shape.
///
/// A series with no scene mapping returns `Ok(None)` — Identify then surfaces the
/// absolute number for manual resolution (the library-safety rule), never guesses.
pub struct TvdbSceneMappings<F: Fetcher = ReqwestFetcher> {
    tvdb: TheTvdbSource<F>,
    configured: bool,
}

impl TvdbSceneMappings<ReqwestFetcher> {
    /// Build the scene-mapping provider from config (shares the TheTVDB key).
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let configured = config.tvdb.api_key.is_some();
        let tvdb = TheTvdbSource::new(
            ReqwestFetcher::new("thetvdb"),
            config.thetvdb_source_config(),
        );
        Self { tvdb, configured }
    }
}

impl<F: Fetcher> TvdbSceneMappings<F> {
    /// Construct directly from a built TheTVDB source (used by tests over a
    /// recorded fetcher). `configured` is the offline gate.
    #[must_use]
    pub fn from_source(tvdb: TheTvdbSource<F>, configured: bool) -> Self {
        Self { tvdb, configured }
    }
}

#[async_trait]
impl<F: Fetcher> SceneMappingProvider for TvdbSceneMappings<F> {
    type Error = MetaError;

    async fn scene_mapping(
        &self,
        series_external_id: &str,
    ) -> Result<Option<SceneMapping>, Self::Error> {
        // No key → no live TheXEM access. Report "no mapping" so Identify degrades
        // gracefully (offline non-negotiable) instead of failing the daemon.
        if !self.configured {
            return Ok(None);
        }
        let map = self.tvdb.scene_map(series_external_id).await?;
        if map.rules.is_empty() {
            // A series with no scene mapping uses its native numbering; surface
            // None so Identify treats an absolute number as unmapped.
            return Ok(None);
        }
        // Distill each scene rule (absolute_start..=absolute_end @ tvdb_season,
        // fixed episode_offset) into the media crate's range shape (season,
        // start_absolute, length). A rule with no upper bound cannot be expressed
        // as a fixed-length range, so it is skipped rather than guessed.
        let ranges: Vec<SceneRange> = map
            .rules
            .iter()
            .filter_map(|r| {
                let end = r.absolute_end?;
                let length = end.checked_sub(r.absolute_start).map(|d| d + 1)?;
                Some(SceneRange {
                    season: r.tvdb_season,
                    start_absolute: r.absolute_start,
                    length,
                })
            })
            .collect();
        if ranges.is_empty() {
            return Ok(None);
        }
        Ok(Some(SceneMapping {
            series: map
                .tvdb_id
                .unwrap_or_else(|| series_external_id.to_string()),
            ranges,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::Coordinates;
    use cellarr_media::remap_absolute;
    use cellarr_meta::{RecordedFetcher, TheTvdbConfig, TmdbConfig};

    const TVDB_BASE: &str = "https://api4.thetvdb.com/v4";

    fn tvdb_with(fetcher: RecordedFetcher) -> TheTvdbSource<RecordedFetcher> {
        TheTvdbSource::new(
            fetcher,
            TheTvdbConfig {
                api_key: Some("test-key".to_string()),
                ..TheTvdbConfig::default()
            },
        )
    }

    fn tmdb_with(fetcher: RecordedFetcher, key: Option<&str>) -> TmdbSource<RecordedFetcher> {
        TmdbSource::new(
            fetcher,
            TmdbConfig {
                api_key: key.map(str::to_string),
                ..TmdbConfig::default()
            },
        )
    }

    /// With no key configured for either source, every lookup degrades to
    /// `Unavailable` (never an error) — the offline non-negotiable. The movie
    /// reason names the missing TMDb key (the blocked-on-key path).
    #[tokio::test]
    async fn no_key_degrades_to_unavailable() {
        let meta = LiveMetadata::from_sources(
            tvdb_with(RecordedFetcher::new()),
            tmdb_with(RecordedFetcher::new(), None),
            false, // tvdb not configured
            false, // tmdb not configured
        );

        let tv = meta.search(MediaType::Tv, "Breaking Bad").await.unwrap();
        match tv {
            LookupOutcome::Unavailable(reason) => assert!(reason.contains("TheTVDB")),
            other => panic!("expected Unavailable, got {other:?}"),
        }

        let movie = meta.search(MediaType::Movie, "The Matrix").await.unwrap();
        match movie {
            LookupOutcome::Unavailable(reason) => assert!(reason.contains("TMDb")),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    /// A configured TheTVDB source resolves a real candidate over recorded bytes:
    /// the human title and the `tvdb` external id (81189 for Breaking Bad) — not
    /// the echoed search term, not a UUID.
    #[tokio::test]
    async fn tv_lookup_resolves_real_tvdb_id_and_title() {
        let search_body = r#"{"status":"success","data":[
            {"tvdb_id":"81189","name":"Breaking Bad","year":"2008",
             "overview":"A chemistry teacher turns to meth."}
        ]}"#;
        let fetcher = RecordedFetcher::new()
            .with_body(&format!("{TVDB_BASE}/search"), search_body.as_bytes());
        let meta = LiveMetadata::from_sources(
            tvdb_with(fetcher),
            tmdb_with(RecordedFetcher::new(), None),
            true,
            false,
        );

        let outcome = meta.search(MediaType::Tv, "Breaking Bad").await.unwrap();
        let LookupOutcome::Resolved(candidates) = outcome else {
            panic!("expected Resolved, got {outcome:?}");
        };
        let bb = candidates
            .iter()
            .find(|c| c.title.contains("Breaking Bad"))
            .expect("Breaking Bad candidate");
        assert_eq!(bb.external_id("tvdb"), Some("81189"));
        assert_eq!(bb.year, Some(2008));
        // Guard against the fake-green pattern: the title must be the real series
        // name, never the search term echoed or an id.
        assert_eq!(bb.title, "Breaking Bad");
        assert_ne!(bb.title, bb.source_id);
    }

    /// The scene-mapping adapter distills a recorded TheXEM map and drives the
    /// media crate's `remap_absolute`: absolute 3 lands in TVDB season 2 episode 1
    /// (per the recorded map), proving the live TheTVDB+TheXEM remap path end to
    /// end (over recorded bytes — no live source on the test path).
    #[tokio::test]
    async fn scene_mapping_adapter_remaps_absolute_through_thexem() {
        let xem = r#"{"result":"success","data":[
            {"scene":{"season":1,"episode":1,"absolute":1},"tvdb":{"season":1,"episode":1,"absolute":1}},
            {"scene":{"season":1,"episode":2,"absolute":2},"tvdb":{"season":1,"episode":2,"absolute":2}},
            {"scene":{"season":2,"episode":1,"absolute":3},"tvdb":{"season":2,"episode":1,"absolute":3}},
            {"scene":{"season":2,"episode":2,"absolute":4},"tvdb":{"season":2,"episode":2,"absolute":4}}
        ]}"#;
        let fetcher =
            RecordedFetcher::new().with_body(&format!("{TVDB_BASE}/xem/map/all"), xem.as_bytes());
        let provider = TvdbSceneMappings::from_source(tvdb_with(fetcher), true);

        let remapped = remap_absolute(&provider, "246521", &Coordinates::Absolute { number: 3 })
            .await
            .expect("absolute 3 should remap");
        assert_eq!(
            remapped,
            Coordinates::Episode {
                season: 2,
                episode: 1,
                absolute: Some(3),
            }
        );
    }

    /// A series with no scene mapping (the recorder 404s the xem route) surfaces
    /// `None`, so Identify treats the absolute number as unmapped (manual
    /// resolution) rather than guessing — the library-safety rule.
    #[tokio::test]
    async fn scene_mapping_absent_is_none() {
        let provider = TvdbSceneMappings::from_source(tvdb_with(RecordedFetcher::new()), true);
        let mapping = provider.scene_mapping("999").await.unwrap();
        assert!(mapping.is_none());
    }
}
