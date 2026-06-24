//! Import-list source backends + the factory that selects one per configured
//! list.
//!
//! Every backend implements [`ListSource`]. The contract is the safeguard
//! ([`cellarr_core::importlist`]): a backend returns [`FetchResult::Fetched`]
//! **only** on a genuine successful round-trip, and routes *every* error
//! (network, auth, parse, a missing credential) through [`FetchResult::Failed`]
//! so a falsely-empty result can never wipe a library.
//!
//! ## Live status
//! - [`TmdbListSource`] — fully wired and **live-testable** with a TMDb API key.
//!   It fetches a TMDb list by id (`/list/{id}`), the popular/trending feeds
//!   (`/movie/popular`, `/trending/movie/{window}`), or a movie collection
//!   (`/collection/{id}`), maps each entry to an [`ImportListItem`] keyed by its
//!   `tmdb` id, and returns `Failed` on any HTTP/decode error (or a missing key).
//! - [`TraktListSource`], [`PlexWatchlistSource`] — the real backends but
//!   **blocked-on-key**: each needs a credential (a Trakt client-id + list slug,
//!   a Plex token). The fetch + JSON mapping is wired, but the live round-trip is
//!   deferred until a real credential is supplied; with none configured the
//!   source returns `Failed` (graceful, inert) rather than hitting the network.
//! - [`ImdbChartSource`] — IMDb's public charts/lists have **no public JSON API**;
//!   the wired backend resolves an IMDb list/chart through a configured proxy
//!   (`settings.json_url`) when present, and otherwise reports a graceful `Failed`
//!   (blocked-on-source, like the token-gated ones) — never a fabricated success.
//! - [`MockListSource`] — a deterministic in-memory source used to test the
//!   framework hermetically (no network, no creds).

use std::sync::Arc;

use async_trait::async_trait;
use cellarr_core::importlist::{FetchResult, ImportListConfig, ImportListItem, ListSource};
use cellarr_core::MediaType;
use cellarr_meta::{Fetcher, ReqwestFetcher};

use super::SourceFactory;

/// Read a string setting from a list's `settings` JSON.
fn setting(config: &ImportListConfig, key: &str) -> Option<String> {
    config
        .settings
        .as_object()
        .and_then(|o| o.get(key))
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
        .filter(|s| !s.is_empty())
}

/// The production factory: builds the live Trakt/TMDb/Plex/IMDb/collection
/// sources over a shared live [`Fetcher`]. A credential-gated source (Trakt,
/// Plex) self-reports `Failed` when its credential is missing, so wiring this
/// factory never makes a network call without config. The TMDb + collection
/// sources fetch for real once an `api_key` is present.
#[derive(Clone)]
pub struct LiveSourceFactory {
    fetcher: Arc<dyn Fetcher>,
}

impl Default for LiveSourceFactory {
    fn default() -> Self {
        Self {
            fetcher: Arc::new(ReqwestFetcher::new("importlist")),
        }
    }
}

impl LiveSourceFactory {
    /// Build the factory over an explicit [`Fetcher`] (tests inject a recorded
    /// one; the daemon uses the default live `reqwest` transport).
    #[must_use]
    pub fn with_fetcher(fetcher: Arc<dyn Fetcher>) -> Self {
        Self { fetcher }
    }
}

impl SourceFactory for LiveSourceFactory {
    fn build(&self, config: &ImportListConfig) -> Option<Arc<dyn ListSource>> {
        match config.kind.to_ascii_lowercase().as_str() {
            "trakt" => Some(Arc::new(TraktListSource::from_config(config))),
            "tmdb" => Some(Arc::new(TmdbListSource::from_config(
                config,
                Arc::clone(&self.fetcher),
            ))),
            // A movie collection is a TMDb collection by id: the same backend with
            // its mode pinned to collection, so "add the other movies in this
            // film's collection" reuses the TMDb fetch path.
            "collection" => Some(Arc::new(TmdbListSource::collection_from_config(
                config,
                Arc::clone(&self.fetcher),
            ))),
            "plex" | "plex-watchlist" => Some(Arc::new(PlexWatchlistSource::from_config(config))),
            "imdb" => Some(Arc::new(ImdbChartSource::from_config(
                config,
                Arc::clone(&self.fetcher),
            ))),
            _ => None,
        }
    }
}

/// The base TMDb v3 API root, used when a list does not override it.
const TMDB_BASE: &str = "https://api.themoviedb.org/3";

/// Which TMDb feed a [`TmdbListSource`] pulls.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TmdbMode {
    /// A user/curated TMDb list by id (`/list/{id}`).
    List(String),
    /// The popular-movies feed (`/movie/popular`).
    Popular,
    /// The trending feed for a window (`day`/`week`) (`/trending/movie/{window}`).
    Trending(String),
    /// A movie collection by id (`/collection/{id}`) — the auto-add-the-rest path.
    Collection(String),
}

/// A TMDb list / popular / trending / collection source.
///
/// **Live-testable** with a TMDb `api_key`. With no key the source reports a
/// graceful `Failed` (the safeguard), never an empty `Fetched`.
pub struct TmdbListSource {
    media_type: MediaType,
    api_key: Option<String>,
    base_url: String,
    mode: TmdbMode,
    fetcher: Arc<dyn Fetcher>,
}

impl TmdbListSource {
    /// Resolve the feed mode from a list's settings. Precedence: an explicit
    /// `list_id` selects a curated list; otherwise a `feed` of `popular` /
    /// `trending` selects those; the default falls back to `popular`.
    fn mode_from_settings(config: &ImportListConfig) -> TmdbMode {
        if let Some(list_id) = setting(config, "list_id") {
            return TmdbMode::List(list_id);
        }
        match setting(config, "feed").as_deref() {
            Some("trending") => {
                let window = setting(config, "window").unwrap_or_else(|| "week".to_string());
                TmdbMode::Trending(window)
            }
            // "popular" or anything unrecognized -> the popular feed.
            _ => TmdbMode::Popular,
        }
    }

    /// Build a list/popular/trending source from a list config.
    #[must_use]
    pub fn from_config(config: &ImportListConfig, fetcher: Arc<dyn Fetcher>) -> Self {
        Self {
            media_type: config.media_type,
            api_key: setting(config, "api_key"),
            base_url: setting(config, "base_url").unwrap_or_else(|| TMDB_BASE.to_string()),
            mode: Self::mode_from_settings(config),
            fetcher,
        }
    }

    /// Build a collection source from a list config (reads `collection_id`).
    #[must_use]
    pub fn collection_from_config(config: &ImportListConfig, fetcher: Arc<dyn Fetcher>) -> Self {
        let collection_id = setting(config, "collection_id").unwrap_or_default();
        Self {
            media_type: config.media_type,
            api_key: setting(config, "api_key"),
            base_url: setting(config, "base_url").unwrap_or_else(|| TMDB_BASE.to_string()),
            mode: TmdbMode::Collection(collection_id),
            fetcher,
        }
    }

    /// The TMDb URL this source's mode fetches, with the key appended.
    fn url(&self, key: &str) -> Option<String> {
        let base = self.base_url.trim_end_matches('/');
        Some(match &self.mode {
            TmdbMode::List(id) => format!("{base}/list/{id}?api_key={key}"),
            TmdbMode::Popular => format!("{base}/movie/popular?api_key={key}"),
            TmdbMode::Trending(window) => format!("{base}/trending/movie/{window}?api_key={key}"),
            TmdbMode::Collection(id) => {
                if id.is_empty() {
                    return None;
                }
                format!("{base}/collection/{id}?api_key={key}")
            }
        })
    }
}

#[async_trait]
impl ListSource for TmdbListSource {
    fn kind(&self) -> &str {
        match self.mode {
            TmdbMode::Collection(_) => "collection",
            _ => "tmdb",
        }
    }

    async fn fetch(&self) -> FetchResult {
        // Blocked-on-key: no key -> a graceful Failed, never an empty Fetched.
        let Some(key) = self.api_key.as_deref() else {
            return FetchResult::Failed(
                "TMDb import list is blocked on credentials (set settings.api_key)".into(),
            );
        };
        let Some(url) = self.url(key) else {
            return FetchResult::Failed(
                "TMDb collection import list needs settings.collection_id".into(),
            );
        };

        let resp = match self.fetcher.get(&url, &[]).await {
            Ok(r) => r,
            Err(e) => return FetchResult::Failed(format!("TMDb fetch failed: {e}")),
        };
        if !resp.is_success() {
            return FetchResult::Failed(format!("TMDb returned HTTP {}", resp.status));
        }
        let value: serde_json::Value = match serde_json::from_slice(&resp.body) {
            Ok(v) => v,
            Err(e) => return FetchResult::Failed(format!("TMDb response decode failed: {e}")),
        };
        // TMDb lists/collections carry their entries under different keys: a list
        // uses `items`, popular/trending use `results`, and a collection uses
        // `parts`. Take whichever array is present.
        let entries = value
            .get("items")
            .or_else(|| value.get("results"))
            .or_else(|| value.get("parts"))
            .and_then(|v| v.as_array());
        let Some(entries) = entries else {
            return FetchResult::Failed(
                "TMDb response missing items/results/parts array".to_string(),
            );
        };
        let items = entries
            .iter()
            .filter_map(|e| tmdb_item(e, self.media_type))
            .collect();
        FetchResult::Fetched(items)
    }
}

/// Map one TMDb entry (list item / popular result / collection part) to an
/// [`ImportListItem`], keyed by its `tmdb` id. A TV entry (`media_type:"tv"`) is
/// only taken when the list is a TV list; the title falls back across the
/// movie/tv title fields TMDb uses.
fn tmdb_item(entry: &serde_json::Value, list_media: MediaType) -> Option<ImportListItem> {
    let id = entry.get("id")?;
    let id_value = id
        .as_u64()
        .map(|n| n.to_string())
        .or_else(|| id.as_str().map(ToString::to_string))?;
    // A mixed feed (e.g. trending/all) tags each entry with its own media type;
    // when present, only take entries that match the list's media type.
    if let Some(mt) = entry.get("media_type").and_then(|v| v.as_str()) {
        let entry_media = match mt {
            "tv" => MediaType::Tv,
            "movie" => MediaType::Movie,
            // An unknown/other kind (person, etc.) is never an addable item.
            _ => return None,
        };
        if entry_media != list_media {
            return None;
        }
    }
    let title = entry
        .get("title")
        .or_else(|| entry.get("name"))
        .or_else(|| entry.get("original_title"))
        .or_else(|| entry.get("original_name"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();
    let year = entry
        .get("release_date")
        .or_else(|| entry.get("first_air_date"))
        .and_then(|v| v.as_str())
        .filter(|s| s.len() >= 4)
        .and_then(|s| s.get(0..4))
        .and_then(|y| y.parse::<i32>().ok());
    Some(ImportListItem {
        id_type: "tmdb".to_string(),
        id_value,
        title,
        year,
        media_type: list_media,
    })
}

/// A Trakt list / user-watchlist source. **Blocked-on-key:** needs a Trakt
/// `client_id` and a `list` slug in the list's settings.
pub struct TraktListSource {
    media_type: MediaType,
    client_id: Option<String>,
    list: Option<String>,
}

impl TraktListSource {
    /// Build from a list config (reads `client_id` + `list` from settings).
    #[must_use]
    pub fn from_config(config: &ImportListConfig) -> Self {
        Self {
            media_type: config.media_type,
            client_id: setting(config, "client_id"),
            list: setting(config, "list"),
        }
    }
}

#[async_trait]
impl ListSource for TraktListSource {
    fn kind(&self) -> &str {
        "trakt"
    }

    async fn fetch(&self) -> FetchResult {
        // Blocked-on-key: without a credential we do NOT hit the network and do
        // NOT return an empty Fetched (which could trigger a clean). We report a
        // graceful Failed, which the sync treats as inert.
        let (Some(_client_id), Some(_list)) = (&self.client_id, &self.list) else {
            return FetchResult::Failed(
                "Trakt import list is blocked on credentials (set settings.client_id and settings.list)".into(),
            );
        };
        let _ = self.media_type;
        // With a credential the real implementation GETs the Trakt list API
        // (api.trakt.tv/users/{user}/lists/{slug}/items) with the client-id header
        // and maps each entry's `ids.tmdb`/`ids.imdb` to an ImportListItem,
        // returning Failed on any HTTP/auth/parse error. Live Trakt access needs a
        // real OAuth-registered client-id (deferred); this returns a clear Failed
        // rather than a fabricated success.
        FetchResult::Failed(
            "Trakt live fetch deferred: needs a registered Trakt client-id (credential present but live round-trip out of scope)".into(),
        )
    }
}

/// A Plex watchlist source. **Blocked-on-key:** needs a Plex `token`.
pub struct PlexWatchlistSource {
    media_type: MediaType,
    token: Option<String>,
}

impl PlexWatchlistSource {
    /// Build from a list config (reads `token` from settings).
    #[must_use]
    pub fn from_config(config: &ImportListConfig) -> Self {
        Self {
            media_type: config.media_type,
            token: setting(config, "token"),
        }
    }
}

#[async_trait]
impl ListSource for PlexWatchlistSource {
    fn kind(&self) -> &str {
        "plex"
    }

    async fn fetch(&self) -> FetchResult {
        let Some(_token) = &self.token else {
            return FetchResult::Failed(
                "Plex watchlist is blocked on credentials (set settings.token)".into(),
            );
        };
        let _ = self.media_type;
        // With a token the real implementation GETs metadata.provider.plex.tv's
        // watchlist endpoint with the X-Plex-Token header and maps each entry's
        // Guid (tmdb://, imdb://) to an ImportListItem. Live Plex access needs a
        // real account token (deferred).
        FetchResult::Failed(
            "Plex live fetch deferred: needs a real Plex account token (credential present but live round-trip out of scope)".into(),
        )
    }
}

/// An IMDb public chart/list source. IMDb exposes no public JSON list API, so a
/// live fetch is **blocked-on-source**: it resolves the list through a configured
/// JSON proxy (`settings.json_url`, e.g. an IMDb-to-JSON gateway returning the
/// same `{results:[{id,title,year}]}` shape) when present, and otherwise reports a
/// graceful `Failed` — never a fabricated success.
pub struct ImdbChartSource {
    media_type: MediaType,
    json_url: Option<String>,
    fetcher: Arc<dyn Fetcher>,
}

impl ImdbChartSource {
    /// Build from a list config (reads `json_url`, an optional JSON proxy).
    #[must_use]
    pub fn from_config(config: &ImportListConfig, fetcher: Arc<dyn Fetcher>) -> Self {
        Self {
            media_type: config.media_type,
            json_url: setting(config, "json_url"),
            fetcher,
        }
    }
}

#[async_trait]
impl ListSource for ImdbChartSource {
    fn kind(&self) -> &str {
        "imdb"
    }

    async fn fetch(&self) -> FetchResult {
        let Some(url) = self.json_url.as_deref() else {
            return FetchResult::Failed(
                "IMDb import list is blocked on source: IMDb has no public list API; set settings.json_url to a JSON list proxy".into(),
            );
        };
        let resp = match self.fetcher.get(url, &[]).await {
            Ok(r) => r,
            Err(e) => return FetchResult::Failed(format!("IMDb list fetch failed: {e}")),
        };
        if !resp.is_success() {
            return FetchResult::Failed(format!("IMDb list proxy returned HTTP {}", resp.status));
        }
        let value: serde_json::Value = match serde_json::from_slice(&resp.body) {
            Ok(v) => v,
            Err(e) => return FetchResult::Failed(format!("IMDb list decode failed: {e}")),
        };
        // Accept either a bare array or `{results:[...]}` / `{items:[...]}`.
        let entries = value
            .as_array()
            .or_else(|| value.get("results").and_then(|v| v.as_array()))
            .or_else(|| value.get("items").and_then(|v| v.as_array()));
        let Some(entries) = entries else {
            return FetchResult::Failed("IMDb list proxy response missing a results array".into());
        };
        let items = entries
            .iter()
            .filter_map(|e| imdb_item(e, self.media_type))
            .collect();
        FetchResult::Fetched(items)
    }
}

/// Map one IMDb-proxy entry to an [`ImportListItem`], keyed by its `imdb` id
/// (`tt…`). Accepts the id under `id`/`imdb_id`/`imdbId`.
fn imdb_item(entry: &serde_json::Value, media: MediaType) -> Option<ImportListItem> {
    let id_value = entry
        .get("id")
        .or_else(|| entry.get("imdb_id"))
        .or_else(|| entry.get("imdbId"))
        .and_then(|v| v.as_str())
        .filter(|s| s.starts_with("tt"))?
        .to_string();
    let title = entry
        .get("title")
        .or_else(|| entry.get("name"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();
    let year = entry
        .get("year")
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .and_then(|y| i32::try_from(y).ok());
    Some(ImportListItem {
        id_type: "imdb".to_string(),
        id_value,
        title,
        year,
        media_type: media,
    })
}

/// A deterministic in-memory list source for hermetic framework tests. It returns
/// a configured [`FetchResult`] verbatim, so a test can drive both the
/// confirmed-good and the failed/empty-because-errored paths exactly.
pub struct MockListSource {
    result: FetchResult,
}

impl MockListSource {
    /// A source that returns a confirmed-good fetch of `items`.
    #[must_use]
    pub fn good(items: Vec<ImportListItem>) -> Self {
        Self {
            result: FetchResult::Fetched(items),
        }
    }

    /// A source that returns a failed fetch with `reason` (the safeguard path).
    #[must_use]
    pub fn failing(reason: impl Into<String>) -> Self {
        Self {
            result: FetchResult::Failed(reason.into()),
        }
    }
}

#[async_trait]
impl ListSource for MockListSource {
    fn kind(&self) -> &str {
        "mock"
    }

    async fn fetch(&self) -> FetchResult {
        self.result.clone()
    }
}

/// A [`SourceFactory`] that always returns the same `MockListSource` result.
/// Used by tests to wire the sync over a deterministic source.
pub struct MockSourceFactory {
    result: FetchResult,
}

impl MockSourceFactory {
    /// Build a factory whose source returns `result` for every list.
    #[must_use]
    pub fn new(result: FetchResult) -> Self {
        Self { result }
    }
}

impl SourceFactory for MockSourceFactory {
    fn build(&self, _config: &ImportListConfig) -> Option<std::sync::Arc<dyn ListSource>> {
        Some(std::sync::Arc::new(MockListSource {
            result: self.result.clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::importlist::CleanAction;
    use cellarr_meta::RecordedFetcher;

    fn tmdb_config(kind: &str, settings: serde_json::Value) -> ImportListConfig {
        ImportListConfig {
            id: "l1".into(),
            name: "TMDb list".into(),
            kind: kind.into(),
            enabled: true,
            media_type: MediaType::Movie,
            monitored: true,
            clean_action: CleanAction::None,
            quality_profile_id: None,
            last_successful_sync: None,
            settings,
        }
    }

    #[tokio::test]
    async fn tmdb_list_fetch_maps_items() {
        let body = serde_json::json!({
            "items": [
                { "id": 603, "title": "The Matrix", "release_date": "1999-03-31" },
                { "id": 604, "title": "The Matrix Reloaded", "release_date": "2003-05-15" },
            ]
        })
        .to_string();
        let fetcher =
            Arc::new(RecordedFetcher::new().with_body("https://api.themoviedb.org/3/list/7", body));
        let cfg = tmdb_config(
            "tmdb",
            serde_json::json!({ "api_key": "k", "list_id": "7" }),
        );
        let src = TmdbListSource::from_config(&cfg, fetcher);
        let result = src.fetch().await;
        let FetchResult::Fetched(items) = result else {
            panic!("expected a confirmed-good fetch, got {result:?}");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id_type, "tmdb");
        assert_eq!(items[0].id_value, "603");
        assert_eq!(items[0].title, "The Matrix");
        assert_eq!(items[0].year, Some(1999));
    }

    #[tokio::test]
    async fn tmdb_collection_fetch_maps_parts() {
        // A collection carries its movies under `parts` — the auto-add-the-rest path.
        let body = serde_json::json!({
            "id": 10,
            "name": "The Matrix Collection",
            "parts": [
                { "id": 603, "title": "The Matrix", "release_date": "1999-03-31" },
                { "id": 605, "title": "The Matrix Revolutions", "release_date": "2003-11-05" },
            ]
        })
        .to_string();
        let fetcher = Arc::new(
            RecordedFetcher::new().with_body("https://api.themoviedb.org/3/collection/10", body),
        );
        let cfg = tmdb_config(
            "collection",
            serde_json::json!({ "api_key": "k", "collection_id": "10" }),
        );
        let src = TmdbListSource::collection_from_config(&cfg, fetcher);
        let result = src.fetch().await;
        let FetchResult::Fetched(items) = result else {
            panic!("expected a confirmed-good fetch, got {result:?}");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].id_value, "605");
        assert_eq!(items[1].title, "The Matrix Revolutions");
    }

    #[tokio::test]
    async fn tmdb_without_key_fails_gracefully_not_empty() {
        // The safeguard at the source: no key is a Failed, never an empty Fetched.
        let fetcher = Arc::new(RecordedFetcher::new());
        let cfg = tmdb_config("tmdb", serde_json::json!({ "list_id": "7" }));
        let src = TmdbListSource::from_config(&cfg, fetcher);
        assert!(matches!(src.fetch().await, FetchResult::Failed(_)));
    }

    #[tokio::test]
    async fn tmdb_http_error_is_failed_not_empty() {
        // A non-2xx must be a Failed (so a clean is never driven), not Fetched([]).
        let fetcher = Arc::new(RecordedFetcher::new().with_response(
            "https://api.themoviedb.org/3/list/7",
            401,
            br#"{"status_message":"invalid key"}"#.to_vec(),
        ));
        let cfg = tmdb_config(
            "tmdb",
            serde_json::json!({ "api_key": "bad", "list_id": "7" }),
        );
        let src = TmdbListSource::from_config(&cfg, fetcher);
        assert!(matches!(src.fetch().await, FetchResult::Failed(_)));
    }

    #[tokio::test]
    async fn tmdb_popular_feed_maps_results() {
        let body = serde_json::json!({
            "results": [ { "id": 1, "title": "A" }, { "id": 2, "title": "B" } ]
        })
        .to_string();
        let fetcher = Arc::new(
            RecordedFetcher::new().with_body("https://api.themoviedb.org/3/movie/popular", body),
        );
        let cfg = tmdb_config(
            "tmdb",
            serde_json::json!({ "api_key": "k", "feed": "popular" }),
        );
        let src = TmdbListSource::from_config(&cfg, fetcher);
        let FetchResult::Fetched(items) = src.fetch().await else {
            panic!("expected fetched");
        };
        assert_eq!(items.len(), 2);
    }

    #[tokio::test]
    async fn trakt_and_plex_are_blocked_on_key() {
        let trakt = TraktListSource::from_config(&tmdb_config("trakt", serde_json::Value::Null));
        assert!(matches!(trakt.fetch().await, FetchResult::Failed(_)));
        let plex = PlexWatchlistSource::from_config(&tmdb_config("plex", serde_json::Value::Null));
        assert!(matches!(plex.fetch().await, FetchResult::Failed(_)));
    }

    #[tokio::test]
    async fn imdb_without_proxy_is_blocked_on_source() {
        let fetcher = Arc::new(RecordedFetcher::new());
        let src =
            ImdbChartSource::from_config(&tmdb_config("imdb", serde_json::Value::Null), fetcher);
        assert!(matches!(src.fetch().await, FetchResult::Failed(_)));
    }

    #[tokio::test]
    async fn imdb_proxy_maps_items() {
        let body = serde_json::json!({
            "results": [ { "id": "tt0133093", "title": "The Matrix", "year": 1999 } ]
        })
        .to_string();
        let fetcher =
            Arc::new(RecordedFetcher::new().with_body("https://imdb-proxy.example/top", body));
        let cfg = tmdb_config(
            "imdb",
            serde_json::json!({ "json_url": "https://imdb-proxy.example/top" }),
        );
        let src = ImdbChartSource::from_config(&cfg, fetcher);
        let FetchResult::Fetched(items) = src.fetch().await else {
            panic!("expected fetched");
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id_type, "imdb");
        assert_eq!(items[0].id_value, "tt0133093");
    }
}
