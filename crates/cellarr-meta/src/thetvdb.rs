//! The TheTVDB v4 metadata source (TV).
//!
//! Implements [`cellarr_core::MetadataSource`] for television, normalizing
//! TheTVDB's JSON into [`Metadata`] with a season/episode child structure.
//! TheTVDB is the one hard, paid external dependency (`docs/07-metadata-service.md`):
//! it has no offline dumps, so the source caches hard and reports
//! [`MetaError::NoCredential`] when no key is configured.
//!
//! Auth in the real API is a bearer token obtained from `POST /login` with the
//! configured `apikey` (and, for the user-supported model, a subscriber `pin`).
//! The token is cached for the lifetime of the source and reused across requests;
//! on a 401 it is dropped and re-minted once. For the record/replay path the
//! [`Fetcher`] seam serves recorded GET bodies, so the adapter's testable
//! normalization logic runs without a token exchange.

use async_trait::async_trait;
use cellarr_core::{MediaType, MetadataSource};
use tokio::sync::Mutex;

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
    /// The cached bearer token from `/login`, minted lazily on first use and
    /// reused thereafter. `None` until the first successful login. Guarded by an
    /// async mutex so concurrent callers mint at most one token.
    token: Mutex<Option<String>>,
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
            token: Mutex::new(None),
        }
    }

    /// Whether a credential is configured at all (offline degradation gate).
    #[must_use]
    pub fn has_credential(&self) -> bool {
        self.config.api_key.is_some()
    }

    /// Exchange the configured api key (+ optional pin) for a bearer token via
    /// `POST /v4/login`, caching it for reuse. Re-login overwrites the cache.
    ///
    /// The api key and pin are sent only in the request body and never logged.
    async fn login(&self) -> Result<String, MetaError> {
        let api_key = self
            .config
            .api_key
            .as_deref()
            .ok_or(MetaError::NoCredential { src: SOURCE })?;

        let mut body = serde_json::Map::new();
        body.insert("apikey".to_string(), serde_json::Value::from(api_key));
        if let Some(pin) = self.config.pin.as_deref().filter(|p| !p.is_empty()) {
            body.insert("pin".to_string(), serde_json::Value::from(pin));
        }
        let body = serde_json::Value::Object(body);

        let url = format!("{}/login", self.config.base_url);
        self.limiter.until_ready().await;
        let resp = self
            .fetcher
            .post_json(&url, &body, &[("Content-Type", "application/json")])
            .await?;
        if !resp.is_success() {
            return Err(MetaError::Http {
                src: SOURCE,
                status: resp.status,
            });
        }
        let parsed: serde_json::Value =
            serde_json::from_slice(&resp.body).map_err(|e| MetaError::Decode {
                src: SOURCE,
                detail: e.to_string(),
            })?;
        let token = parsed
            .get("data")
            .and_then(|d| d.get("token"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| MetaError::Decode {
                src: SOURCE,
                detail: "login response missing data.token".to_string(),
            })?
            .to_string();
        Ok(token)
    }

    /// Return a cached token, minting one via [`login`](Self::login) if absent.
    async fn bearer(&self) -> Result<String, MetaError> {
        let mut guard = self.token.lock().await;
        if let Some(tok) = guard.as_ref() {
            return Ok(tok.clone());
        }
        let tok = self.login().await?;
        *guard = Some(tok.clone());
        Ok(tok)
    }

    /// Drop the cached token so the next request re-logs in (used after a 401).
    async fn invalidate_token(&self) {
        *self.token.lock().await = None;
    }

    async fn cached_get(&self, cache_key: &str, url: String) -> Result<String, MetaError> {
        self.cache
            .get_or_try_insert_with(cache_key, async { self.authed_get(&url).await })
            .await
    }

    /// A single authenticated GET that retries once on a 401 by re-minting the
    /// token (handles expiry/rotation transparently).
    async fn authed_get(&self, url: &str) -> Result<String, MetaError> {
        let token = self.bearer().await?;
        let auth = format!("Bearer {token}");
        self.limiter.until_ready().await;
        let resp = self
            .fetcher
            .get(url, &[("Authorization", auth.as_str())])
            .await?;

        let resp = if resp.status == 401 {
            // Token expired/invalid: drop it, log in again, retry once.
            self.invalidate_token().await;
            let token = self.bearer().await?;
            let auth = format!("Bearer {token}");
            self.limiter.until_ready().await;
            self.fetcher
                .get(url, &[("Authorization", auth.as_str())])
                .await?
        } else {
            resp
        };

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

    /// Fetch the default-order English episode list for a series.
    ///
    /// The `/extended` payload carries identity, remote ids, and artwork but not
    /// always the full episode list, so episodes are fetched from the dedicated
    /// `/series/{id}/episodes/default/eng` endpoint and merged. A 404/empty here
    /// is not fatal — the series identity still normalizes.
    async fn fetch_episodes(&self, id: &str) -> Vec<ChildNode> {
        let url = format!(
            "{}/series/{}/episodes/default/eng",
            self.config.base_url, id
        );
        let body = match self.cached_get(&format!("tvdb:episodes:{id}"), url).await {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };
        let value: serde_json::Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        // The episodes endpoint nests the list under `data.episodes`.
        value
            .get("data")
            .and_then(|d| d.get("episodes"))
            .and_then(|e| e.as_array())
            .map(|eps| eps.iter().filter_map(normalize_episode).collect())
            .unwrap_or_default()
    }

    /// Fetch and normalize a series with its season/episode child structure.
    ///
    /// Combines `/series/{id}/extended` (identity, ids, artwork) with the
    /// default-order English episode list; the episode endpoint is only consulted
    /// when `/extended` did not already carry episodes (the recorded fixtures
    /// embed them).
    ///
    /// # Errors
    /// As [`TheTvdbSource::search_normalized`].
    pub async fn fetch_normalized(&self, id: &str) -> Result<Metadata, MetaError> {
        let value = self.fetch_raw(id).await?;
        let mut meta = normalize_series(&value).ok_or_else(|| MetaError::Decode {
            src: SOURCE,
            detail: "series response missing required fields".to_string(),
        })?;
        if meta.children.is_empty() {
            meta.children = self.fetch_episodes(id).await;
        }
        Ok(meta)
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

    // TheTVDB `/series/{id}/extended` carries `aliases: [{language, name}]` — the
    // alternate/romanized/English titles a non-English series is also filed under.
    // Keep the distinct non-empty names (primary title excluded) so an English-named
    // library file can still match a Japanese-titled anime.
    let mut aliases: Vec<String> = Vec::new();
    if let Some(arr) = data.get("aliases").and_then(serde_json::Value::as_array) {
        for a in arr {
            if let Some(name) = a.get("name").and_then(|v| v.as_str()) {
                let name = name.trim();
                if !name.is_empty() && name != title && !aliases.iter().any(|x| x == name) {
                    aliases.push(name.to_string());
                }
            }
        }
    }

    let images = collect_artworks(data.get("artworks"));

    // TheTVDB `/series/{id}/extended` carries `genres: [{id, name, slug}]`.
    let genres = data
        .get("genres")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|g| g.get("name").and_then(|v| v.as_str()))
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(Metadata {
        source_id,
        media_type: MediaType::Tv,
        title,
        aliases,
        year,
        overview: data
            .get("overview")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        // TheTVDB series-level `averageRuntime` is the typical episode length in
        // minutes; surfaced as the series runtime when present and non-zero.
        runtime: data
            .get("averageRuntime")
            .and_then(serde_json::Value::as_u64)
            .filter(|&n| n > 0)
            .and_then(|n| u32::try_from(n).ok()),
        // The series' first-air date is its "release" date for our schema.
        release_date: data
            .get("firstAired")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        // No separate digital release for a series.
        digital_release: None,
        external_ids,
        children,
        images,
        genres,
        // TheTVDB's `score` is a popularity metric, not a 0–10 user rating, so we
        // leave the rating unset rather than surface a non-comparable number.
        rating: None,
        rating_votes: None,
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
