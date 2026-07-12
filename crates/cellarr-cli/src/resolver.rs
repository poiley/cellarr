//! The live metadata resolver — the concrete [`MetadataResolver`] the
//! identify/refresh path resolves a node's rich metadata and artwork through.
//!
//! There was no concrete resolver before this: the trait existed and the pipeline
//! held an `Option<Arc<dyn DynMetadataResolver>>`, but nothing implemented it, so
//! `RefreshMetadata` was a no-op and added movies never gained overview, runtime,
//! genres, ratings, or posters. This fills that gap.
//!
//! Given a content node it:
//!   1. looks up the node's native external id (`tmdb` for movies, `tvdb` for TV);
//!   2. fetches the source's full details;
//!   3. maps them into the persisted [`ContentMetadata`] (overview / runtime /
//!      dates / genres / rating);
//!   4. caches the poster + fanart bytes under the MediaCover artwork dir.
//!
//! Degradation is non-negotiable: an unidentified node, or one whose source has no
//! API key, resolves to `Ok(None)` (nothing to persist) rather than an error.
//! Artwork fetches are best-effort — a failed image never fails the resolve.

use std::path::PathBuf;

use async_trait::async_trait;

use cellarr_core::{ContentMetadata, ContentRef, MediaType};
use cellarr_db::Database;
use cellarr_media::{ArtworkKind, MetadataResolver, ResolvedMetadata};
use cellarr_meta::{Fetcher, Image, Metadata, ReqwestFetcher, TheTvdbSource, TmdbSource};

use crate::config::Config;

/// How many times to attempt an artwork image download before treating it as a
/// miss. One initial try plus two retries turns most transient CDN blips (a TMDB
/// 5xx or connection reset under load) into a hit, so a freshly added film is not
/// left posterless until the next daily refresh.
const ARTWORK_FETCH_ATTEMPTS: u32 = 3;

/// The resolver's error: a genuine provider/repository failure the refresh caller
/// logs and moves past (a metadata refresh never blocks acquisition).
#[derive(Debug, thiserror::Error)]
pub enum ResolverError {
    /// A metadata source request failed (network/decoding), key present.
    #[error("metadata source error: {0}")]
    Source(String),
    /// A repository read failed while looking up the node's external id.
    #[error("repository error: {0}")]
    Repo(String),
}

/// The live `MetadataResolver`, holding the concrete TheTVDB (TV) and TMDb (movie)
/// sources plus the DB handle (to resolve a node's external id) and the artwork
/// fetcher + cache dir.
///
/// Generic over the [`Fetcher`] so the daemon binds the real `reqwest` transport
/// while tests bind a recorded one (the same mapping + artwork-caching logic runs
/// in both). `artwork_fetcher` is a separate fetcher used only for image bytes so
/// the recorded test path can supply artwork without a live host.
pub struct LiveMetadataResolver<F: Fetcher = ReqwestFetcher> {
    db: Database,
    tvdb: TheTvdbSource<F>,
    tmdb: TmdbSource<F>,
    tvdb_configured: bool,
    tmdb_configured: bool,
    artwork_fetcher: F,
    artwork_dir: PathBuf,
}

impl LiveMetadataResolver<ReqwestFetcher> {
    /// Build the live resolver from the loaded daemon config, the DB handle, and
    /// the MediaCover artwork dir. Keys come from `CELLARR_TVDB__*` / `CELLARR_TMDB__*`;
    /// an absent key leaves the corresponding source unavailable (degraded).
    #[must_use]
    pub fn from_config(config: &Config, db: Database, artwork_dir: PathBuf) -> Self {
        let tvdb_configured = config.tvdb.api_key.is_some();
        let tmdb_configured = config.tmdb.api_key.is_some();
        let tvdb = TheTvdbSource::new(
            ReqwestFetcher::new("thetvdb"),
            config.thetvdb_source_config(),
        );
        let tmdb = TmdbSource::new(ReqwestFetcher::new("tmdb"), config.tmdb_source_config());
        Self {
            db,
            tvdb,
            tmdb,
            tvdb_configured,
            tmdb_configured,
            artwork_fetcher: ReqwestFetcher::new("artwork"),
            artwork_dir,
        }
    }
}

impl<F: Fetcher> LiveMetadataResolver<F> {
    /// Construct directly from built sources (used by tests over a recorded
    /// fetcher). `*_configured` are the offline gates.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn from_sources(
        db: Database,
        tvdb: TheTvdbSource<F>,
        tmdb: TmdbSource<F>,
        tvdb_configured: bool,
        tmdb_configured: bool,
        artwork_fetcher: F,
        artwork_dir: PathBuf,
    ) -> Self {
        Self {
            db,
            tvdb,
            tmdb,
            tvdb_configured,
            tmdb_configured,
            artwork_fetcher,
            artwork_dir,
        }
    }

    /// Fetch a node's full details from the source matching its media type, or
    /// `None` when that source has no key (degraded — never an error).
    async fn fetch_details(
        &self,
        media_type: MediaType,
        external_id: &str,
    ) -> Result<Option<Metadata>, ResolverError> {
        match media_type {
            MediaType::Movie if self.tmdb_configured => self
                .tmdb
                .fetch_normalized(external_id)
                .await
                .map(Some)
                .map_err(|e| ResolverError::Source(e.to_string())),
            MediaType::Tv if self.tvdb_configured => self
                .tvdb
                .fetch_normalized(external_id)
                .await
                .map(Some)
                .map_err(|e| ResolverError::Source(e.to_string())),
            _ => Ok(None),
        }
    }

    /// Cache the poster/fanart bytes under `<artwork_dir>/<content_id>/<kind>.jpg`
    /// and return which kinds were successfully cached. Best-effort: a failed
    /// download or write is skipped, never an error.
    ///
    /// The image fetch is **retried** ([`ARTWORK_FETCH_ATTEMPTS`]) before giving
    /// up: a single transient blip from the image CDN otherwise means a freshly
    /// added film silently has no poster until the next daily refresh — the
    /// "added a film, artwork never showed up" gap. A genuine miss (every attempt
    /// failed, or the provider offered no art) is logged at `warn`, not swallowed
    /// at `debug`, so a persistent problem is visible.
    async fn cache_artwork(&self, content_id: &str, images: &[Image]) -> Vec<ArtworkKind> {
        let mut kinds = Vec::new();
        let dir = self.artwork_dir.join(content_id);
        for image in images {
            let kind = match image.kind.as_str() {
                "poster" => ArtworkKind::Poster,
                "fanart" => ArtworkKind::Fanart,
                _ => continue,
            };
            let Some(bytes) = self.fetch_artwork_bytes(&image.url).await else {
                tracing::warn!(url = %image.url, kind = %kind.slug(), content = %content_id, "artwork fetch gave up after retries");
                continue;
            };
            if let Err(e) = tokio::fs::create_dir_all(&dir).await {
                tracing::debug!(dir = %dir.display(), error = %e, "artwork dir create failed");
                continue;
            }
            let path = dir.join(format!("{}.jpg", kind.slug()));
            match tokio::fs::write(&path, &bytes).await {
                Ok(()) => kinds.push(kind),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "artwork write failed")
                }
            }
        }
        kinds
    }

    /// Fetch one artwork image's bytes, retrying a transient failure (network
    /// error, non-2xx, empty body) up to [`ARTWORK_FETCH_ATTEMPTS`] with a short
    /// backoff. Returns `None` only when every attempt failed — the caller treats
    /// that as a (logged) miss. Image CDNs (TMDB) occasionally 5xx or reset under
    /// load; one retry turns most of those into a hit.
    async fn fetch_artwork_bytes(&self, url: &str) -> Option<Vec<u8>> {
        for attempt in 1..=ARTWORK_FETCH_ATTEMPTS {
            match self.artwork_fetcher.get(url, &[]).await {
                Ok(resp) if (200..300).contains(&resp.status) && !resp.body.is_empty() => {
                    return Some(resp.body);
                }
                Ok(resp) => {
                    tracing::debug!(url = %url, status = resp.status, attempt, "artwork fetch non-2xx/empty");
                }
                Err(e) => {
                    tracing::debug!(url = %url, error = %e, attempt, "artwork fetch failed");
                }
            }
            if attempt < ARTWORK_FETCH_ATTEMPTS {
                tokio::time::sleep(std::time::Duration::from_millis(500 * u64::from(attempt)))
                    .await;
            }
        }
        None
    }
}

/// Project a source's normalized [`Metadata`] into the persisted [`ContentMetadata`].
fn to_content_metadata(m: Metadata) -> ContentMetadata {
    ContentMetadata {
        title: Some(m.title),
        year: m.year,
        overview: m.overview,
        runtime: m.runtime,
        air_date: m.release_date,
        digital_date: m.digital_release,
        genres: m.genres,
        rating: m.rating,
        rating_votes: m.rating_votes,
    }
}

#[async_trait]
impl<F: Fetcher> MetadataResolver for LiveMetadataResolver<F> {
    type Error = ResolverError;

    #[tracing::instrument(
        name = "metadata.resolve",
        skip_all,
        fields(content_id = %content.id, media_type = ?content.media_type)
    )]
    async fn resolve(&self, content: &ContentRef) -> Result<Option<ResolvedMetadata>, Self::Error> {
        // The node's native external id (tmdb for movies, tvdb for TV). An
        // unidentified node has none — nothing to resolve.
        let external = self
            .db
            .content()
            .external_id_for(content.id, content.media_type)
            .await
            .map_err(|e| ResolverError::Repo(e.to_string()))?;
        let Some((scheme, external_id)) = external else {
            return Ok(None);
        };
        // Only the native scheme drives a details fetch — an imdb-only node cannot
        // be fetched by id from the tmdb/tvdb details endpoints.
        let native = matches!(
            (content.media_type, scheme.as_str()),
            (MediaType::Movie, "tmdb") | (MediaType::Tv, "tvdb")
        );
        if !native {
            return Ok(None);
        }
        let Some(meta) = self.fetch_details(content.media_type, &external_id).await? else {
            return Ok(None);
        };
        let images = meta.images.clone();
        let resolved_meta = to_content_metadata(meta);
        let artwork = self.cache_artwork(&content.id.to_string(), &images).await;
        Ok(Some(ResolvedMetadata {
            meta: resolved_meta,
            artwork,
        }))
    }
}
