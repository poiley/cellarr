//! cellarr-download — download-client integrations.
//!
//! [`cellarr_core::DownloadClient`] implementations per client with a uniform
//! add → track → complete → remove lifecycle (see `docs/specs/cellarr-download.md`
//! and `docs/06-integrations.md`). v1 ships:
//!
//! - [`QbittorrentClient`] — torrent, WebUI API v2, version-aware `SID` login.
//! - [`SabnzbdClient`] — Usenet, `mode=` HTTP API + `apikey=` + `output=json`.
//! - [`NzbgetClient`] — Usenet, JSON-RPC positional params + HTTP Basic.
//! - [`BlackholeClient`] — the *universal* client: a watch/completed folder pair
//!   that works with **any** download client (no client API at all). See
//!   [`blackhole`].
//!
//! # Design: one narrow HTTP seam, richer-than-core status
//!
//! Every adapter routes its I/O through [`HttpTransport`] so contract tests can
//! replay recorded API exchanges with no live client (the integration-test rule
//! in `docs/06-integrations.md`). Production uses [`ReqwestTransport`].
//!
//! `cellarr-core`'s [`cellarr_core::DownloadStatus`] now carries the detail the
//! pipeline executor needs — coarse [`cellarr_core::DownloadState`], on-disk
//! `content_path` for Import, `progress`, and the seed `ratio`/`seeding_time_secs`
//! for gated removal. Adapters compute all of that into a richer crate-local
//! [`DownloadProgress`] (which also tracks the client `category` core does not
//! model) and project it onto the trait return type via
//! [`DownloadProgress::to_core_status`], so the trait alone gives the executor a
//! full picture without downcasting to a concrete client.
//!
//! # Category scoping
//!
//! Every `add` tags the download with cellarr's category so cellarr only ever
//! touches its own downloads; [`DownloadProgress::is_in_category`] lets callers
//! refuse to act on a foreign download.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod blackhole;
pub mod error;
pub mod http;
pub mod lifecycle;
pub mod nzbget;
pub mod qbittorrent;
pub mod sabnzbd;

pub use blackhole::{BlackholeClient, BlackholeSettings};
pub use error::DownloadError;
pub use http::{HttpRequest, HttpResponse, HttpTransport, ReqwestTransport};
pub use lifecycle::{DownloadProgress, RemovePolicy};
pub use nzbget::{NzbgetClient, NzbgetSettings};
pub use qbittorrent::{QbittorrentClient, QbittorrentSettings};
pub use sabnzbd::{SabnzbdClient, SabnzbdSettings};

use async_trait::async_trait;
use cellarr_core::{DownloadClient, DownloadStatus, GrabRequest};

#[async_trait]
impl DownloadClient for QbittorrentClient {
    type Error = DownloadError;

    fn name(&self) -> &str {
        QbittorrentClient::name(self)
    }

    async fn add(&self, grab: &GrabRequest) -> Result<String, Self::Error> {
        QbittorrentClient::add(self, grab).await
    }

    async fn status(&self, download_id: &str) -> Result<DownloadStatus, Self::Error> {
        // Project the adapter's richer view onto the core status so callers get
        // content_path/progress/seed signals straight off the trait.
        Ok(QbittorrentClient::progress(self, download_id)
            .await?
            .to_core_status())
    }

    async fn remove(&self, download_id: &str, delete_data: bool) -> Result<(), Self::Error> {
        // The core trait's `remove` is unconditional; ratio/time-gated removal is
        // the richer `QbittorrentClient::remove(_, RemovePolicy)`. Here we honour
        // the caller's explicit delete intent immediately.
        QbittorrentClient::remove(self, download_id, RemovePolicy::immediate(delete_data)).await?;
        Ok(())
    }
}

#[async_trait]
impl DownloadClient for SabnzbdClient {
    type Error = DownloadError;

    fn name(&self) -> &str {
        SabnzbdClient::name(self)
    }

    async fn add(&self, grab: &GrabRequest) -> Result<String, Self::Error> {
        SabnzbdClient::add(self, grab).await
    }

    async fn status(&self, download_id: &str) -> Result<DownloadStatus, Self::Error> {
        Ok(SabnzbdClient::progress(self, download_id)
            .await?
            .to_core_status())
    }

    async fn remove(&self, download_id: &str, delete_data: bool) -> Result<(), Self::Error> {
        SabnzbdClient::remove(self, download_id, delete_data).await
    }
}

#[async_trait]
impl DownloadClient for BlackholeClient {
    type Error = DownloadError;

    fn name(&self) -> &str {
        BlackholeClient::name(self)
    }

    async fn add(&self, grab: &GrabRequest) -> Result<String, Self::Error> {
        BlackholeClient::add(self, grab).await
    }

    async fn status(&self, download_id: &str) -> Result<DownloadStatus, Self::Error> {
        Ok(BlackholeClient::progress(self, download_id)
            .await?
            .to_core_status())
    }

    async fn remove(&self, download_id: &str, delete_data: bool) -> Result<(), Self::Error> {
        BlackholeClient::remove(self, download_id, delete_data).await
    }
}

#[async_trait]
impl DownloadClient for NzbgetClient {
    type Error = DownloadError;

    fn name(&self) -> &str {
        NzbgetClient::name(self)
    }

    async fn add(&self, grab: &GrabRequest) -> Result<String, Self::Error> {
        NzbgetClient::add(self, grab).await
    }

    async fn status(&self, download_id: &str) -> Result<DownloadStatus, Self::Error> {
        Ok(NzbgetClient::progress(self, download_id)
            .await?
            .to_core_status())
    }

    async fn remove(&self, download_id: &str, delete_data: bool) -> Result<(), Self::Error> {
        NzbgetClient::remove(self, download_id, delete_data).await
    }
}
