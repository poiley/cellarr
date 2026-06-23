//! Source configuration (bring-your-own-key).
//!
//! Users may configure their own TMDb/TheTVDB credentials for a fully private
//! setup, or point at a shared default instance, or run with none at all — and
//! the daemon must still start and degrade gracefully (offline is
//! non-negotiable, `docs/07-metadata-service.md`). These structs carry the
//! per-source knobs; absent credentials are represented as `None`, never as a
//! startup failure.

use std::time::Duration;

/// Configuration for the TMDb source (movies, + TV imagery).
#[derive(Debug, Clone)]
pub struct TmdbConfig {
    /// The v4 read-access bearer token, or v3 API key. `None` means "no key" →
    /// the source reports [`MetaError::NoCredential`] and the daemon treats it
    /// as unavailable.
    ///
    /// [`MetaError::NoCredential`]: crate::MetaError::NoCredential
    pub api_key: Option<String>,
    /// API base; overridable for a self-hosted/shared default proxy. Defaults to
    /// the public TMDb v3 base.
    pub base_url: String,
    /// How long fetched records stay fresh in cache. Metadata changes slowly, so
    /// this is generous by default.
    pub cache_ttl: Duration,
    /// Conservative requests-per-second ceiling (TMDb's documented soft limit is
    /// tens/s; we stay well under and honor 429s).
    pub rate_per_second: u32,
}

impl Default for TmdbConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            base_url: "https://api.themoviedb.org/3".to_string(),
            cache_ttl: Duration::from_secs(24 * 60 * 60),
            rate_per_second: 4,
        }
    }
}

/// Configuration for the TheTVDB v4 source (TV).
#[derive(Debug, Clone)]
pub struct TheTvdbConfig {
    /// The licensed API key (or user key). `None` means "no key" → unavailable.
    pub api_key: Option<String>,
    /// Optional subscriber PIN for user-supplied keys.
    pub pin: Option<String>,
    /// API base; overridable for a self-hosted/shared default proxy. Defaults to
    /// the public TheTVDB v4 base.
    pub base_url: String,
    /// Cache TTL for fetched records.
    pub cache_ttl: Duration,
    /// Conservative requests-per-second ceiling (TheTVDB's limit is unpublished,
    /// so we are deliberately cautious).
    pub rate_per_second: u32,
}

impl Default for TheTvdbConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            pin: None,
            base_url: "https://api4.thetvdb.com/v4".to_string(),
            cache_ttl: Duration::from_secs(24 * 60 * 60),
            rate_per_second: 2,
        }
    }
}
