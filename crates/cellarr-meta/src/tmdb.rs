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
    ///
    /// First searches the term verbatim. If that returns nothing **and** the term
    /// ends in a 4-digit year (e.g. `"Dune 2021"`), it retries with the year moved
    /// to TMDb's dedicated `&year=` filter (`query=Dune&year=2021`) — TMDb's text
    /// search treats `"Dune 2021"` as a literal title and finds nothing, while the
    /// split form resolves it. The fallback is **retry-on-empty**, so a year that
    /// is genuinely part of a title (`"Blade Runner 2049"`, `"1917"`) is never
    /// stripped — those return results on the first pass and the retry never fires.
    async fn search_raw(&self, query: &str) -> Result<serde_json::Value, MetaError> {
        let value = self.search_raw_inner(query, None).await?;
        if search_has_results(&value) {
            return Ok(value);
        }
        if let (title, Some(year)) = split_trailing_year(query) {
            return self.search_raw_inner(title, Some(year)).await;
        }
        Ok(value)
    }

    /// One TMDb `search/movie` call (optionally with a `&year=` filter).
    async fn search_raw_inner(
        &self,
        title: &str,
        year: Option<u16>,
    ) -> Result<serde_json::Value, MetaError> {
        let key = self.api_key()?;
        let mut url = format!(
            "{}/search/movie?api_key={}&query={}",
            self.config.base_url,
            key,
            urlencode(title)
        );
        if let Some(y) = year {
            url.push_str(&format!("&year={y}"));
        }
        let body = self
            .cached_get(&format!("tmdb:search:{title}:{year:?}"), url)
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

/// Whether a TMDb `search/movie` payload has at least one result.
fn search_has_results(value: &serde_json::Value) -> bool {
    value
        .get("results")
        .and_then(|r| r.as_array())
        .is_some_and(|a| !a.is_empty())
}

/// Split a trailing 4-digit year off a search term: `"Dune 2021"` → `("Dune",
/// Some(2021))`. Returns `(term, None)` when the term has no multi-word trailing
/// year (so a bare `"2012"` / `"1917"` title is left intact — there must be a
/// title before the year). Only used as a retry when the verbatim search was
/// empty, so a year that is part of a title never gets stripped in practice.
fn split_trailing_year(term: &str) -> (&str, Option<u16>) {
    let trimmed = term.trim_end();
    let Some(idx) = trimmed.rfind(' ') else {
        return (term, None);
    };
    let (head, last) = trimmed.split_at(idx);
    let last = last.trim();
    let head = head.trim_end();
    if head.is_empty() || last.len() != 4 || !last.bytes().all(|b| b.is_ascii_digit()) {
        return (term, None);
    }
    match last.parse::<u16>() {
        Ok(y) if (1900..=2100).contains(&y) => (head, Some(y)),
        _ => (term, None),
    }
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

    // Genres: TMDb details carries `genres: [{id, name}]`; keep the names in order.
    let genres = value
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
    // Rating: `vote_average` (0..10) with `vote_count` votes. A zero count means
    // no ratings yet — drop the rating so the UI shows nothing rather than 0.0.
    #[allow(clippy::cast_possible_truncation)]
    let rating_votes = value
        .get("vote_count")
        .and_then(serde_json::Value::as_u64)
        .filter(|&n| n > 0)
        .and_then(|n| u32::try_from(n).ok());
    #[allow(clippy::cast_possible_truncation)]
    let rating = rating_votes.and_then(|_| {
        value
            .get("vote_average")
            .and_then(serde_json::Value::as_f64)
            .filter(|&v| v > 0.0)
            .map(|v| v as f32)
    });

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
        // TMDb `runtime` is in whole minutes; 0 means unknown (drop it).
        runtime: value
            .get("runtime")
            .and_then(serde_json::Value::as_u64)
            .filter(|&n| n > 0)
            .and_then(|n| u32::try_from(n).ok()),
        release_date: iso_date(value.get("release_date")),
        // The digital release is in `release_dates` (type 4) when present; the
        // base movie payload does not carry it, so it stays absent here until the
        // append_to_response gathers it. Modeled so a future fetch can populate it.
        digital_release: None,
        external_ids,
        children: Vec::new(),
        images,
        genres,
        rating,
        rating_votes,
    })
}

/// Return an ISO `yyyy-mm-dd` date value as an owned string when it is a
/// non-empty string, else `None`.
fn iso_date(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The TMDb image CDN base. TMDb serves artwork from this host (the `file_path`
/// values in the API are host-relative); `original` is the always-valid full-size
/// rendition. This base is stable and documented; we compose it rather than call
/// the `/configuration` endpoint for a single fixed value.
const TMDB_IMAGE_BASE: &str = "https://image.themoviedb.org/t/p/original";

fn collect_images(images: &serde_json::Value) -> Vec<Image> {
    let mut out = Vec::new();
    // Take only the primary (first, i.e. highest-voted) poster and fanart — the
    // resolver caches one of each kind. Compose the full CDN URL from the
    // host-relative `file_path` so callers get a directly resolvable URL.
    for (kind, key) in [("poster", "posters"), ("fanart", "backdrops")] {
        if let Some(path) = images
            .get(key)
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|img| img.get("file_path"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            out.push(Image {
                kind: kind.to_string(),
                url: format!("{TMDB_IMAGE_BASE}{path}"),
            });
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

#[cfg(test)]
mod year_split_tests {
    use super::split_trailing_year;

    #[test]
    fn splits_a_trailing_release_year() {
        assert_eq!(split_trailing_year("Dune 2021"), ("Dune", Some(2021)));
        assert_eq!(
            split_trailing_year("The Matrix 1999"),
            ("The Matrix", Some(1999))
        );
    }

    #[test]
    fn leaves_year_that_is_the_whole_title_or_in_title() {
        // A bare numeric title has no preceding title -> untouched.
        assert_eq!(split_trailing_year("2012"), ("2012", None));
        assert_eq!(split_trailing_year("1917"), ("1917", None));
        // No trailing year at all.
        assert_eq!(split_trailing_year("Inception"), ("Inception", None));
        // Out of plausible range.
        assert_eq!(split_trailing_year("Title 3000"), ("Title 3000", None));
    }
}

#[cfg(test)]
mod normalize_movie_tests {
    use super::normalize_movie;
    use serde_json::json;

    /// A full TMDb movie-details payload normalizes the rich fields: genres,
    /// rating (vote_average + vote_count), and the primary poster + fanart composed
    /// into full CDN URLs (host-relative `file_path` -> absolute).
    #[test]
    fn parses_genres_rating_and_full_image_urls() {
        let payload = json!({
            "id": 10378,
            "title": "Big Buck Bunny",
            "overview": "A big rabbit takes revenge on three rodents.",
            "runtime": 10,
            "release_date": "2008-05-20",
            "vote_average": 7.5,
            "vote_count": 1234,
            "genres": [{"id": 16, "name": "Animation"}, {"id": 35, "name": "Comedy"}],
            "images": {
                "posters": [{"file_path": "/poster1.jpg"}, {"file_path": "/poster2.jpg"}],
                "backdrops": [{"file_path": "/back1.jpg"}]
            }
        });
        let meta = normalize_movie(&payload).expect("movie normalizes");
        assert_eq!(
            meta.overview.as_deref(),
            Some("A big rabbit takes revenge on three rodents.")
        );
        assert_eq!(meta.runtime, Some(10));
        assert_eq!(
            meta.genres,
            vec!["Animation".to_string(), "Comedy".to_string()]
        );
        assert_eq!(meta.rating, Some(7.5));
        assert_eq!(meta.rating_votes, Some(1234));
        // Only the primary poster + fanart, each a full CDN URL.
        assert_eq!(meta.images.len(), 2);
        let poster = meta.images.iter().find(|i| i.kind == "poster").unwrap();
        assert_eq!(
            poster.url,
            "https://image.themoviedb.org/t/p/original/poster1.jpg"
        );
        let fanart = meta.images.iter().find(|i| i.kind == "fanart").unwrap();
        assert_eq!(
            fanart.url,
            "https://image.themoviedb.org/t/p/original/back1.jpg"
        );
    }

    /// A movie with no votes yet drops the rating entirely (so the UI shows nothing
    /// rather than a misleading 0.0).
    #[test]
    fn zero_votes_drops_the_rating() {
        let payload = json!({ "id": 1, "title": "Unrated", "vote_average": 0.0, "vote_count": 0 });
        let meta = normalize_movie(&payload).unwrap();
        assert_eq!(meta.rating, None);
        assert_eq!(meta.rating_votes, None);
    }
}
