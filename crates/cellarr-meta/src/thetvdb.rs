//! The TheTVDB v4 metadata source (TV).
//!
//! Implements [`cellarr_core::MetadataSource`] for television, normalizing
//! TheTVDB's JSON into [`Metadata`] with a season/episode child structure.
//! TheTVDB is the one hard, paid external dependency (`docs/07-metadata-service.md`):
//! it has no offline dumps, so the source caches hard and reports
//! [`MetaError::NoCredential`] when no key is configured.
//!
//! Auth in the real API is a bearer token obtained from `/login`; for the
//! record/replay path the [`Fetcher`] seam serves recorded bodies, so the
//! adapter's testable normalization logic runs without a token exchange. The
//! configured key is sent as a bearer header on live requests.

use async_trait::async_trait;
use cellarr_core::{MediaType, MetadataSource};

use crate::cache::MetaCache;
use crate::config::TheTvdbConfig;
use crate::error::MetaError;
use crate::http::Fetcher;
use crate::normalized::{ChildNode, Image, Metadata, SearchResult};
use crate::ratelimit::RateLimiter;
use crate::scene::{parse_xem, SceneMap};

const SOURCE: &str = "thetvdb";

/// A TheTVDB adapter over an injected [`Fetcher`] (live or recorded).
pub struct TheTvdbSource<F: Fetcher> {
    fetcher: F,
    config: TheTvdbConfig,
    cache: MetaCache,
    limiter: RateLimiter,
}

impl<F: Fetcher> TheTvdbSource<F> {
    /// Construct a TheTVDB source from a fetcher and config.
    #[must_use]
    pub fn new(fetcher: F, config: TheTvdbConfig) -> Self {
        let cache = MetaCache::new(config.cache_ttl, 10_000);
        let limiter = RateLimiter::per_second(config.rate_per_second);
        Self {
            fetcher,
            config,
            cache,
            limiter,
        }
    }

    fn bearer(&self) -> Result<String, MetaError> {
        self.config
            .api_key
            .as_deref()
            .map(|k| format!("Bearer {k}"))
            .ok_or(MetaError::NoCredential { src: SOURCE })
    }

    async fn cached_get(&self, cache_key: &str, url: String) -> Result<String, MetaError> {
        let bearer = self.bearer()?;
        self.cache
            .get_or_try_insert_with(cache_key, async {
                self.limiter.until_ready().await;
                let resp = self
                    .fetcher
                    .get(&url, &[("Authorization", bearer.as_str())])
                    .await?;
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

    async fn search_raw(&self, query: &str) -> Result<serde_json::Value, MetaError> {
        let url = format!(
            "{}/search?type=series&query={}",
            self.config.base_url,
            urlencode(query)
        );
        let body = self
            .cached_get(&format!("tvdb:search:{query}"), url)
            .await?;
        serde_json::from_str(&body).map_err(|e| MetaError::Decode {
            src: SOURCE,
            detail: e.to_string(),
        })
    }

    /// Search TheTVDB and normalize results.
    ///
    /// # Errors
    /// [`MetaError::NoCredential`] with no key; HTTP/transport/decode otherwise.
    pub async fn search_normalized(&self, query: &str) -> Result<Vec<SearchResult>, MetaError> {
        let value = self.search_raw(query).await?;
        let data = value
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| MetaError::Decode {
                src: SOURCE,
                detail: "search response missing 'data' array".to_string(),
            })?;
        Ok(data.iter().filter_map(normalize_search_item).collect())
    }

    async fn fetch_raw(&self, id: &str) -> Result<serde_json::Value, MetaError> {
        let url = format!("{}/series/{}/extended", self.config.base_url, id);
        let body = self.cached_get(&format!("tvdb:fetch:{id}"), url).await?;
        serde_json::from_str(&body).map_err(|e| MetaError::Decode {
            src: SOURCE,
            detail: e.to_string(),
        })
    }

    /// Fetch and normalize a series with its season/episode child structure.
    ///
    /// # Errors
    /// As [`TheTvdbSource::search_normalized`].
    pub async fn fetch_normalized(&self, id: &str) -> Result<Metadata, MetaError> {
        let value = self.fetch_raw(id).await?;
        normalize_series(&value).ok_or_else(|| MetaError::Decode {
            src: SOURCE,
            detail: "series response missing required fields".to_string(),
        })
    }

    /// Artwork references for a series.
    ///
    /// # Errors
    /// As [`TheTvdbSource::fetch_normalized`].
    pub async fn images(&self, id: &str) -> Result<Vec<Image>, MetaError> {
        Ok(self.fetch_normalized(id).await?.images)
    }

    /// Fetch the TheXEM scene map for a series (anime/scene numbering).
    ///
    /// Returns an empty map when the series has no scene mapping. Used by Identify
    /// to remap [`cellarr_core::Coordinates::Absolute`].
    ///
    /// # Errors
    /// HTTP/transport/decode errors when the mapping source is reachable but
    /// misbehaves.
    pub async fn scene_map(&self, id: &str) -> Result<SceneMap, MetaError> {
        // TheXEM is keyed by TVDB series id; it is a separate, open endpoint, so
        // no bearer is required. A 404 means "no mapping", not an error.
        let url = format!("{}/xem/map/all?id={}&origin=tvdb", self.config.base_url, id);
        self.limiter.until_ready().await;
        let resp = self.fetcher.get(&url, &[]).await?;
        if resp.status == 404 {
            return Ok(SceneMap {
                tvdb_id: Some(id.to_string()),
                ..SceneMap::default()
            });
        }
        if !resp.is_success() {
            return Err(MetaError::Http {
                src: SOURCE,
                status: resp.status,
            });
        }
        parse_xem(&resp.body, Some(id.to_string()))
    }
}

#[async_trait]
impl<F: Fetcher> MetadataSource for TheTvdbSource<F> {
    type Error = MetaError;

    fn media_type(&self) -> MediaType {
        MediaType::Tv
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

    async fn scene_mapping(&self, external_id: &str) -> Result<Vec<serde_json::Value>, MetaError> {
        let map = self.scene_map(external_id).await?;
        Ok(map
            .rules
            .into_iter()
            .map(|r| serde_json::to_value(r).unwrap_or(serde_json::Value::Null))
            .collect())
    }
}

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
    // TheTVDB search items expose the series id as `tvdb_id` (string).
    let source_id = item
        .get("tvdb_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| item.get("id").and_then(json_id))?;
    let title = item.get("name").and_then(|v| v.as_str())?.to_string();
    Some(SearchResult {
        source_id,
        media_type: MediaType::Tv,
        title,
        year: item
            .get("year")
            .and_then(|v| v.as_str())
            .and_then(|y| y.parse().ok()),
        overview: item
            .get("overview")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        external_ids: Vec::new(),
    })
}

fn normalize_series(value: &serde_json::Value) -> Option<Metadata> {
    let data = value.get("data").unwrap_or(value);
    let source_id = data.get("id").and_then(json_id)?;
    let title = data.get("name").and_then(|v| v.as_str())?.to_string();

    let year = data
        .get("year")
        .and_then(|v| v.as_str())
        .and_then(|y| y.parse().ok())
        .or_else(|| year_from_date(data.get("firstAired")));

    let mut external_ids = vec![("tvdb".to_string(), source_id.clone())];
    if let Some(remotes) = data.get("remoteIds").and_then(|v| v.as_array()) {
        for r in remotes {
            if let (Some(src), Some(id)) = (
                r.get("sourceName").and_then(|v| v.as_str()),
                r.get("id").and_then(|v| v.as_str()),
            ) {
                if src.eq_ignore_ascii_case("imdb") {
                    external_ids.push(("imdb".to_string(), id.to_string()));
                }
            }
        }
    }

    let children = data
        .get("episodes")
        .and_then(|v| v.as_array())
        .map(|eps| eps.iter().filter_map(normalize_episode).collect())
        .unwrap_or_default();

    let images = collect_artworks(data.get("artworks"));

    Some(Metadata {
        source_id,
        media_type: MediaType::Tv,
        title,
        year,
        overview: data
            .get("overview")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        external_ids,
        children,
        images,
    })
}

fn normalize_episode(ep: &serde_json::Value) -> Option<ChildNode> {
    Some(ChildNode {
        source_id: ep.get("id").and_then(json_id),
        season: ep
            .get("seasonNumber")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32),
        episode: ep.get("number").and_then(|v| v.as_u64()).map(|n| n as u32),
        absolute: ep
            .get("absoluteNumber")
            .and_then(|v| v.as_u64())
            .filter(|n| *n > 0)
            .map(|n| n as u32),
        air_date: ep
            .get("aired")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        title: ep
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

fn collect_artworks(artworks: Option<&serde_json::Value>) -> Vec<Image> {
    let Some(arr) = artworks.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|a| {
            let url = a.get("image").and_then(|v| v.as_str())?;
            // TheTVDB artwork `type` is a numeric enum; map the common poster
            // (2) and background (3) codes, fall back to a generic label.
            let kind = match a.get("type").and_then(|v| v.as_u64()) {
                Some(2) => "poster",
                Some(3) => "fanart",
                _ => "image",
            };
            Some(Image {
                kind: kind.to_string(),
                url: url.to_string(),
            })
        })
        .collect()
}

/// TheTVDB ids appear as either a JSON number or a numeric string; accept both.
fn json_id(v: &serde_json::Value) -> Option<String> {
    v.as_u64()
        .map(|n| n.to_string())
        .or_else(|| v.as_str().map(str::to_string))
}

fn year_from_date(date: Option<&serde_json::Value>) -> Option<u16> {
    date.and_then(|v| v.as_str())
        .filter(|s| s.len() >= 4)
        .and_then(|s| s.get(0..4))
        .and_then(|y| y.parse().ok())
}
