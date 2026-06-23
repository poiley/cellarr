//! The TMDb metadata source (movies).
//!
//! Implements [`cellarr_core::MetadataSource`] for movies, normalizing TMDb's
//! JSON into the crate's [`Metadata`]/[`SearchResult`] schema. Auth is the v3
//! `api_key` query parameter (BYO key); with no key the source reports
//! [`MetaError::NoCredential`] and the daemon degrades gracefully.
//!
//! Every request goes through the per-source [`RateLimiter`] and the [`MetaCache`]
//! so repeated lookups are cheap and TMDb's soft rate limit is respected.

use async_trait::async_trait;
use cellarr_core::{MediaType, MetadataSource};

use crate::cache::MetaCache;
use crate::config::TmdbConfig;
use crate::error::MetaError;
use crate::http::Fetcher;
use crate::normalized::{Image, Metadata, SearchResult};
use crate::ratelimit::RateLimiter;

const SOURCE: &str = "tmdb";

/// A TMDb adapter over an injected [`Fetcher`] (live or recorded).
pub struct TmdbSource<F: Fetcher> {
    fetcher: F,
    config: TmdbConfig,
    cache: MetaCache,
    limiter: RateLimiter,
}

impl<F: Fetcher> TmdbSource<F> {
    /// Construct a TMDb source from a fetcher and config.
    #[must_use]
    pub fn new(fetcher: F, config: TmdbConfig) -> Self {
        let cache = MetaCache::new(config.cache_ttl, 10_000);
        let limiter = RateLimiter::per_second(config.rate_per_second);
        Self {
            fetcher,
            config,
            cache,
            limiter,
        }
    }

    fn api_key(&self) -> Result<&str, MetaError> {
        self.config
            .api_key
            .as_deref()
            .ok_or(MetaError::NoCredential { src: SOURCE })
    }

    /// Fetch a URL through the cache + limiter, returning the raw body string.
    async fn cached_get(&self, cache_key: &str, url: String) -> Result<String, MetaError> {
        self.cache
            .get_or_try_insert_with(cache_key, async {
                self.limiter.until_ready().await;
                let resp = self.fetcher.get(&url, &[]).await?;
                if !resp.is_success() {
                    return Err(MetaError::Http {
                        src: SOURCE,
                        status: resp.status,
                    });
                }
                String::from_utf8(resp.body).map_err(|e| MetaError::Decode {
                    src: SOURCE,
                    detail: e.to_string(),
                })
            })
            .await
    }

    /// The raw search payload (used by both `search` and the trait's JSON form).
    async fn search_raw(&self, query: &str) -> Result<serde_json::Value, MetaError> {
        let key = self.api_key()?;
        let url = format!(
            "{}/search/movie?api_key={}&query={}",
            self.config.base_url,
            key,
            urlencode(query)
        );
        let body = self
            .cached_get(&format!("tmdb:search:{query}"), url)
            .await?;
        serde_json::from_str(&body).map_err(|e| MetaError::Decode {
            src: SOURCE,
            detail: e.to_string(),
        })
    }

    /// Search TMDb and normalize results.
    ///
    /// # Errors
    /// [`MetaError::NoCredential`] with no key; HTTP/transport/decode errors on a
    /// reachable source.
    pub async fn search_normalized(&self, query: &str) -> Result<Vec<SearchResult>, MetaError> {
        let value = self.search_raw(query).await?;
        let results = value
            .get("results")
            .and_then(|r| r.as_array())
            .ok_or_else(|| MetaError::Decode {
                src: SOURCE,
                detail: "search response missing 'results' array".to_string(),
            })?;
        Ok(results.iter().filter_map(normalize_search_item).collect())
    }

    /// The raw fetch payload for a movie id.
    async fn fetch_raw(&self, id: &str) -> Result<serde_json::Value, MetaError> {
        let key = self.api_key()?;
        let url = format!(
            "{}/movie/{}?api_key={}&append_to_response=images",
            self.config.base_url, id, key
        );
        let body = self.cached_get(&format!("tmdb:fetch:{id}"), url).await?;
        serde_json::from_str(&body).map_err(|e| MetaError::Decode {
            src: SOURCE,
            detail: e.to_string(),
        })
    }

    /// Fetch and normalize a movie record.
    ///
    /// # Errors
    /// As [`TmdbSource::search_normalized`].
    pub async fn fetch_normalized(&self, id: &str) -> Result<Metadata, MetaError> {
        let value = self.fetch_raw(id).await?;
        normalize_movie(&value).ok_or_else(|| MetaError::Decode {
            src: SOURCE,
            detail: "movie response missing required fields".to_string(),
        })
    }

    /// Artwork references for a movie.
    ///
    /// # Errors
    /// As [`TmdbSource::fetch_normalized`].
    pub async fn images(&self, id: &str) -> Result<Vec<Image>, MetaError> {
        Ok(self.fetch_normalized(id).await?.images)
    }
}

#[async_trait]
impl<F: Fetcher> MetadataSource for TmdbSource<F> {
    type Error = MetaError;

    fn media_type(&self) -> MediaType {
        MediaType::Movie
    }

    async fn search(&self, query: &str) -> Result<Vec<serde_json::Value>, MetaError> {
        Ok(self
            .search_normalized(query)
            .await?
            .into_iter()
            .map(|r| serde_json::to_value(r).unwrap_or(serde_json::Value::Null))
            .collect())
    }

    async fn fetch(&self, external_id: &str) -> Result<serde_json::Value, MetaError> {
        let meta = self.fetch_normalized(external_id).await?;
        serde_json::to_value(meta).map_err(|e| MetaError::Decode {
            src: SOURCE,
            detail: e.to_string(),
        })
    }

    // Movies carry no scene numbering; the default empty `scene_mapping` applies.
}

/// Minimal percent-encoding for query strings (space + the reserved chars we
/// actually emit). Avoids a dep just for the search path.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn normalize_search_item(item: &serde_json::Value) -> Option<SearchResult> {
    let id = item.get("id")?;
    let source_id = id
        .as_u64()
        .map_or_else(|| id.as_str().map(str::to_string), |n| Some(n.to_string()))?;
    let title = item
        .get("title")
        .or_else(|| item.get("original_title"))
        .and_then(|v| v.as_str())?
        .to_string();
    Some(SearchResult {
        source_id,
        media_type: MediaType::Movie,
        title,
        year: year_from_date(item.get("release_date")),
        overview: item
            .get("overview")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        external_ids: Vec::new(),
    })
}

fn normalize_movie(value: &serde_json::Value) -> Option<Metadata> {
    let id = value.get("id")?;
    let source_id = id
        .as_u64()
        .map_or_else(|| id.as_str().map(str::to_string), |n| Some(n.to_string()))?;
    let title = value
        .get("title")
        .or_else(|| value.get("original_title"))
        .and_then(|v| v.as_str())?
        .to_string();

    let mut external_ids = Vec::new();
    if let Some(imdb) = value.get("imdb_id").and_then(|v| v.as_str()) {
        if !imdb.is_empty() {
            external_ids.push(("imdb".to_string(), imdb.to_string()));
        }
    }
    external_ids.push(("tmdb".to_string(), source_id.clone()));

    let images = value.get("images").map(collect_images).unwrap_or_default();

    Some(Metadata {
        source_id,
        media_type: MediaType::Movie,
        title,
        year: year_from_date(value.get("release_date")),
        overview: value
            .get("overview")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        external_ids,
        children: Vec::new(),
        images,
    })
}

fn collect_images(images: &serde_json::Value) -> Vec<Image> {
    let mut out = Vec::new();
    for (kind, key) in [("poster", "posters"), ("fanart", "backdrops")] {
        if let Some(arr) = images.get(key).and_then(|v| v.as_array()) {
            for img in arr {
                if let Some(path) = img.get("file_path").and_then(|v| v.as_str()) {
                    out.push(Image {
                        kind: kind.to_string(),
                        url: path.to_string(),
                    });
                }
            }
        }
    }
    out
}

/// Extract the leading 4-digit year from an ISO `yyyy-mm-dd` date value.
fn year_from_date(date: Option<&serde_json::Value>) -> Option<u16> {
    date.and_then(|v| v.as_str())
        .filter(|s| s.len() >= 4)
        .and_then(|s| s.get(0..4))
        .and_then(|y| y.parse().ok())
}
