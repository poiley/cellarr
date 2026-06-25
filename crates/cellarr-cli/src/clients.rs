//! The live download-client factory: build a [`DownloadClient`] from a persisted
//! [`DownloadClientConfig`].
//!
//! The pipeline's Grab/Track stages take a single [`cellarr_core::DownloadClient`].
//! A deployment persists [`DownloadClientConfig`] rows (kind + open-ended
//! `settings` JSON: host, credentials, paths) via the db `ConfigRepo`. This module
//! reads the *enabled* client of highest priority at run time and constructs the
//! matching native adapter (qBittorrent / Transmission / Deluge / rTorrent /
//! SABnzbd / NZBGet / Blackhole), wrapping
//! it in [`ConfiguredDownloadClient`] so the runner is driven over one concrete
//! `DownloadClient` whose error type is unified (mirroring the indexer set's
//! `NabAdapter`).
//!
//! Reading the config when the handler is built (rather than caching a live
//! socket) keeps the chosen client in step with CRUD writes: a client added or
//! reconfigured through the API is visible to the next pipeline run with no code
//! change here. Each adapter routes its I/O through the real `reqwest` transport.

use async_trait::async_trait;
use cellarr_core::{DownloadClient, DownloadClientConfig, DownloadStatus, GrabRequest};
use cellarr_download::{
    BlackholeClient, BlackholeSettings, DelugeClient, DelugeSettings, DownloadError, NzbgetClient,
    NzbgetSettings, QbittorrentClient, QbittorrentSettings, RtorrentClient, RtorrentSettings,
    SabnzbdClient, SabnzbdSettings, TransmissionClient, TransmissionSettings,
};

/// A failure building a download client from its persisted configuration.
#[derive(Debug, thiserror::Error)]
pub enum DownloadClientFactoryError {
    /// The configured client's `kind` is not one this build supports.
    #[error("unsupported download client kind '{kind}'")]
    UnsupportedKind {
        /// The unrecognized kind string.
        kind: String,
    },

    /// The client's `settings` JSON was missing or malformed for its kind (e.g.
    /// no `baseUrl`/`apiKey`), so no adapter could be built from it.
    #[error("download client '{name}' is misconfigured: {reason}")]
    Misconfigured {
        /// The configured client's name.
        name: String,
        /// Why the adapter could not be built.
        reason: String,
    },
}

/// One built native download-client adapter, dispatched dynamically by kind.
///
/// Implements [`DownloadClient`] with a single unified [`DownloadError`], so the
/// [`PipelineRunner`](cellarr_jobs::PipelineRunner) (generic over `D:
/// DownloadClient`) is driven over this one type regardless of which client was
/// configured.
pub enum ConfiguredDownloadClient {
    /// qBittorrent (torrent, WebUI v2).
    Qbittorrent(QbittorrentClient),
    /// Transmission (torrent, RPC).
    Transmission(TransmissionClient),
    /// Deluge (torrent, JSON-RPC WebUI).
    Deluge(DelugeClient),
    /// rTorrent (torrent, XML-RPC).
    Rtorrent(RtorrentClient),
    /// SABnzbd (Usenet).
    Sabnzbd(SabnzbdClient),
    /// NZBGet (Usenet, JSON-RPC).
    Nzbget(NzbgetClient),
    /// Blackhole / watch-folder (universal).
    Blackhole(BlackholeClient),
}

impl ConfiguredDownloadClient {
    /// Build the adapter for one [`DownloadClientConfig`], reading its open-ended
    /// `settings` JSON (the shape the API shim persists) into the kind's typed
    /// settings struct.
    ///
    /// # Errors
    /// [`DownloadClientFactoryError::UnsupportedKind`] for a kind this build does
    /// not ship; [`DownloadClientFactoryError::Misconfigured`] when the settings
    /// JSON does not deserialize for the chosen kind.
    pub fn from_config(config: &DownloadClientConfig) -> Result<Self, DownloadClientFactoryError> {
        let kind = config.kind.to_ascii_lowercase();
        let name = config.name.clone();
        let category = config.category.clone();
        let misconfigured = |reason: String| DownloadClientFactoryError::Misconfigured {
            name: name.clone(),
            reason,
        };
        match kind.as_str() {
            "qbittorrent" | "qbit" => {
                let settings: QbittorrentSettings =
                    parse_settings(config).map_err(&misconfigured)?;
                Ok(Self::Qbittorrent(QbittorrentClient::new(
                    name, settings, category,
                )))
            }
            "transmission" | "transmissionbt" => {
                let settings: TransmissionSettings =
                    parse_settings(config).map_err(&misconfigured)?;
                Ok(Self::Transmission(TransmissionClient::new(
                    name, settings, category,
                )))
            }
            "deluge" => {
                let settings: DelugeSettings = parse_settings(config).map_err(&misconfigured)?;
                Ok(Self::Deluge(DelugeClient::new(name, settings, category)))
            }
            "rtorrent" => {
                let settings: RtorrentSettings = parse_settings(config).map_err(&misconfigured)?;
                Ok(Self::Rtorrent(RtorrentClient::new(
                    name, settings, category,
                )))
            }
            "sabnzbd" | "sab" => {
                let settings: SabnzbdSettings = parse_settings(config).map_err(&misconfigured)?;
                Ok(Self::Sabnzbd(SabnzbdClient::new(name, settings, category)))
            }
            "nzbget" => {
                let settings: NzbgetSettings = parse_settings(config).map_err(&misconfigured)?;
                Ok(Self::Nzbget(NzbgetClient::new(name, settings, category)))
            }
            "blackhole" => {
                let settings: BlackholeSettings = parse_settings(config).map_err(&misconfigured)?;
                // The blackhole writes either `.torrent`/`.nzb`/`.magnet` jobs; the
                // protocol it stamps onto a job comes from the client config (a
                // torrent blackhole vs a usenet one).
                Ok(Self::Blackhole(BlackholeClient::new(
                    name,
                    settings,
                    category,
                    config.protocol,
                )))
            }
            other => Err(DownloadClientFactoryError::UnsupportedKind {
                kind: other.to_string(),
            }),
        }
    }
}

/// Deserialize a client's open-ended `settings` JSON into its typed struct,
/// rendering a readable reason on failure.
fn parse_settings<T: serde::de::DeserializeOwned>(
    config: &DownloadClientConfig,
) -> Result<T, String> {
    serde_json::from_value(config.settings.clone())
        .map_err(|e| format!("settings JSON does not match {} schema: {e}", config.kind))
}

#[async_trait]
impl DownloadClient for ConfiguredDownloadClient {
    type Error = DownloadError;

    fn name(&self) -> &str {
        match self {
            Self::Qbittorrent(c) => <QbittorrentClient as DownloadClient>::name(c),
            Self::Transmission(c) => <TransmissionClient as DownloadClient>::name(c),
            Self::Deluge(c) => <DelugeClient as DownloadClient>::name(c),
            Self::Rtorrent(c) => <RtorrentClient as DownloadClient>::name(c),
            Self::Sabnzbd(c) => <SabnzbdClient as DownloadClient>::name(c),
            Self::Nzbget(c) => <NzbgetClient as DownloadClient>::name(c),
            Self::Blackhole(c) => <BlackholeClient as DownloadClient>::name(c),
        }
    }

    async fn add(&self, grab: &GrabRequest) -> Result<String, Self::Error> {
        // Fully-qualified trait calls: each concrete client has inherent methods
        // of the same name (`status`/`remove` with a richer signature), so we must
        // dispatch through the `DownloadClient` trait impl in `cellarr-download`
        // (which projects the rich progress onto the core `DownloadStatus`).
        match self {
            Self::Qbittorrent(c) => DownloadClient::add(c, grab).await,
            Self::Transmission(c) => DownloadClient::add(c, grab).await,
            Self::Deluge(c) => DownloadClient::add(c, grab).await,
            Self::Rtorrent(c) => DownloadClient::add(c, grab).await,
            Self::Sabnzbd(c) => DownloadClient::add(c, grab).await,
            Self::Nzbget(c) => DownloadClient::add(c, grab).await,
            Self::Blackhole(c) => DownloadClient::add(c, grab).await,
        }
    }

    async fn status(&self, download_id: &str) -> Result<DownloadStatus, Self::Error> {
        match self {
            Self::Qbittorrent(c) => DownloadClient::status(c, download_id).await,
            Self::Transmission(c) => DownloadClient::status(c, download_id).await,
            Self::Deluge(c) => DownloadClient::status(c, download_id).await,
            Self::Rtorrent(c) => DownloadClient::status(c, download_id).await,
            Self::Sabnzbd(c) => DownloadClient::status(c, download_id).await,
            Self::Nzbget(c) => DownloadClient::status(c, download_id).await,
            Self::Blackhole(c) => DownloadClient::status(c, download_id).await,
        }
    }

    async fn remove(&self, download_id: &str, delete_data: bool) -> Result<(), Self::Error> {
        match self {
            Self::Qbittorrent(c) => DownloadClient::remove(c, download_id, delete_data).await,
            Self::Transmission(c) => DownloadClient::remove(c, download_id, delete_data).await,
            Self::Deluge(c) => DownloadClient::remove(c, download_id, delete_data).await,
            Self::Rtorrent(c) => DownloadClient::remove(c, download_id, delete_data).await,
            Self::Sabnzbd(c) => DownloadClient::remove(c, download_id, delete_data).await,
            Self::Nzbget(c) => DownloadClient::remove(c, download_id, delete_data).await,
            Self::Blackhole(c) => DownloadClient::remove(c, download_id, delete_data).await,
        }
    }
}

/// A download client that does nothing and is never driven.
///
/// The interactive release-search preview (`GET /api/v3/release`) runs the
/// read-only Discover→Decide path through the [`PipelineRunner`](cellarr_jobs::PipelineRunner),
/// which is generic over a `D: DownloadClient` but **never calls** it for a
/// preview (the preview stops before Grab). The runner still needs *a* client
/// value to construct, so the search path supplies this no-op rather than
/// building a live adapter — which means a misconfigured/unreachable download
/// client can never fail an interactive search (a search never grabs).
///
/// Every method returns a [`DownloadError::Config`] error so that, were it ever
/// driven by mistake, the failure is loud and self-describing rather than a
/// silent success.
pub struct NoopDownloadClient;

#[async_trait]
impl DownloadClient for NoopDownloadClient {
    type Error = DownloadError;

    fn name(&self) -> &str {
        "noop (interactive search; never grabs)"
    }

    async fn add(&self, _grab: &GrabRequest) -> Result<String, Self::Error> {
        Err(DownloadError::Config(
            "interactive release search must not grab".into(),
        ))
    }

    async fn status(&self, _download_id: &str) -> Result<DownloadStatus, Self::Error> {
        Err(DownloadError::Config(
            "interactive release search must not track".into(),
        ))
    }

    async fn remove(&self, _download_id: &str, _delete_data: bool) -> Result<(), Self::Error> {
        Err(DownloadError::Config(
            "interactive release search must not remove a download".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::{DownloadClientId, Protocol};
    use serde_json::json;

    fn config(kind: &str, settings: serde_json::Value) -> DownloadClientConfig {
        DownloadClientConfig {
            tags: Vec::new(),
            id: DownloadClientId::new(),
            name: format!("test-{kind}"),
            kind: kind.to_string(),
            protocol: Protocol::Torrent,
            enabled: true,
            priority: 0,
            category: "cellarr".to_string(),
            settings,
        }
    }

    #[test]
    fn builds_qbittorrent_from_settings() {
        let c = config(
            "qbittorrent",
            json!({"base_url": "http://localhost:8080", "username": "u", "password": "p"}),
        );
        let built = ConfiguredDownloadClient::from_config(&c).expect("qbit builds");
        assert!(matches!(built, ConfiguredDownloadClient::Qbittorrent(_)));
        assert_eq!(DownloadClient::name(&built), "test-qbittorrent");
    }

    #[test]
    fn builds_transmission_from_settings() {
        let c = config(
            "transmission",
            json!({"host": "localhost", "port": 9091, "category": "cellarr-tv"}),
        );
        let built = ConfiguredDownloadClient::from_config(&c).expect("transmission builds");
        assert!(matches!(built, ConfiguredDownloadClient::Transmission(_)));
        assert_eq!(DownloadClient::name(&built), "test-transmission");
    }

    #[test]
    fn builds_transmission_from_base_url() {
        // The simpler base_url shape (no host/port) is also accepted.
        let c = config("transmission", json!({"base_url": "http://localhost:9091"}));
        let built = ConfiguredDownloadClient::from_config(&c).expect("transmission builds");
        assert!(matches!(built, ConfiguredDownloadClient::Transmission(_)));
    }

    #[test]
    fn builds_blackhole_carrying_protocol() {
        let c = config(
            "blackhole",
            json!({"watch_folder": "/w", "completed_folder": "/c"}),
        );
        let built = ConfiguredDownloadClient::from_config(&c).expect("blackhole builds");
        assert!(matches!(built, ConfiguredDownloadClient::Blackhole(_)));
    }

    #[test]
    fn builds_deluge_from_settings() {
        let c = config(
            "deluge",
            json!({"host": "localhost", "port": 8112, "password": "secret"}),
        );
        let built = ConfiguredDownloadClient::from_config(&c).expect("deluge builds");
        assert!(matches!(built, ConfiguredDownloadClient::Deluge(_)));
        assert_eq!(DownloadClient::name(&built), "test-deluge");
    }

    #[test]
    fn builds_rtorrent_from_settings() {
        let c = config(
            "rtorrent",
            json!({"host": "localhost", "port": 8080, "urlBase": "/RPC2"}),
        );
        let built = ConfiguredDownloadClient::from_config(&c).expect("rtorrent builds");
        assert!(matches!(built, ConfiguredDownloadClient::Rtorrent(_)));
        assert_eq!(DownloadClient::name(&built), "test-rtorrent");
    }

    #[test]
    fn unsupported_kind_is_rejected() {
        let c = config("aria2", json!({}));
        // `ConfiguredDownloadClient` is not `Debug` (it holds live transports), so
        // match the result directly rather than `expect_err`.
        assert!(matches!(
            ConfiguredDownloadClient::from_config(&c),
            Err(DownloadClientFactoryError::UnsupportedKind { .. })
        ));
    }

    #[test]
    fn malformed_settings_are_misconfigured() {
        // qbittorrent settings require base_url/username/password.
        let c = config("qbittorrent", json!({"base_url": "http://x"}));
        assert!(matches!(
            ConfiguredDownloadClient::from_config(&c),
            Err(DownloadClientFactoryError::Misconfigured { .. })
        ));
    }
}
