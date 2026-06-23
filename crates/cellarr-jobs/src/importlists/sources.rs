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
//! - [`TraktListSource`], [`TmdbListSource`], [`PlexWatchlistSource`] — the real
//!   backends, but **blocked-on-key**: each needs a credential (a Trakt
//!   client-id + list slug, a TMDb API key + list id, a Plex token). With no
//!   credential in the list's `settings`, the source returns `Failed` (graceful,
//!   inert) rather than hitting the network. This keeps the framework fully wired
//!   and credential-gated, exactly like the metadata/indexer live paths.
//! - [`MockListSource`] — a deterministic in-memory source used to test the
//!   framework hermetically (no network, no creds).

use async_trait::async_trait;
use cellarr_core::importlist::{FetchResult, ImportListConfig, ImportListItem, ListSource};

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

/// The production factory: builds the live (credential-gated) Trakt/TMDb/Plex
/// sources. Each built source self-reports `Failed` when its credential is
/// missing, so wiring this factory never makes a network call without config.
#[derive(Clone, Default)]
pub struct LiveSourceFactory;

impl SourceFactory for LiveSourceFactory {
    fn build(&self, config: &ImportListConfig) -> Option<std::sync::Arc<dyn ListSource>> {
        match config.kind.to_ascii_lowercase().as_str() {
            "trakt" => Some(std::sync::Arc::new(TraktListSource::from_config(config))),
            "tmdb" => Some(std::sync::Arc::new(TmdbListSource::from_config(config))),
            "plex" | "plex-watchlist" => Some(std::sync::Arc::new(
                PlexWatchlistSource::from_config(config),
            )),
            _ => None,
        }
    }
}

/// A Trakt list / user-watchlist source. **Blocked-on-key:** needs a Trakt
/// `client_id` and a `list` slug in the list's settings.
pub struct TraktListSource {
    media_type: cellarr_core::MediaType,
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
        // With a credential the real implementation would GET the Trakt list API
        // and map entries to ImportListItem, returning Failed on any HTTP/auth/parse
        // error. Live Trakt access is out of scope (needs real creds); this returns
        // a clear Failed rather than a fabricated success.
        FetchResult::Failed(
            "Trakt live fetch not enabled in this build (credential present but network access out of scope)".into(),
        )
    }
}

/// A TMDb list source. **Blocked-on-key:** needs a TMDb `api_key` and a `list_id`.
pub struct TmdbListSource {
    media_type: cellarr_core::MediaType,
    api_key: Option<String>,
    list_id: Option<String>,
}

impl TmdbListSource {
    /// Build from a list config (reads `api_key` + `list_id` from settings).
    #[must_use]
    pub fn from_config(config: &ImportListConfig) -> Self {
        Self {
            media_type: config.media_type,
            api_key: setting(config, "api_key"),
            list_id: setting(config, "list_id"),
        }
    }
}

#[async_trait]
impl ListSource for TmdbListSource {
    fn kind(&self) -> &str {
        "tmdb"
    }

    async fn fetch(&self) -> FetchResult {
        let (Some(_api_key), Some(_list_id)) = (&self.api_key, &self.list_id) else {
            return FetchResult::Failed(
                "TMDb import list is blocked on credentials (set settings.api_key and settings.list_id)".into(),
            );
        };
        let _ = self.media_type;
        FetchResult::Failed(
            "TMDb live fetch not enabled in this build (credential present but network access out of scope)".into(),
        )
    }
}

/// A Plex watchlist source. **Blocked-on-key:** needs a Plex `token`.
pub struct PlexWatchlistSource {
    media_type: cellarr_core::MediaType,
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
        FetchResult::Failed(
            "Plex live fetch not enabled in this build (credential present but network access out of scope)".into(),
        )
    }
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
