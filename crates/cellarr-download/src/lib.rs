//! cellarr-download — download-client integrations.
//!
//! [`cellarr_core::DownloadClient`] implementations per client with a uniform
//! add → track → complete → remove lifecycle (see `docs/specs/cellarr-download.md`
//! and `docs/06-integrations.md`). v1 ships:
//!
//! - [`QbittorrentClient`] — torrent, WebUI API v2, version-aware `SID` login.
//! - [`SabnzbdClient`] — Usenet, `mode=` HTTP API + `apikey=` + `output=json`.
//! - [`NzbgetClient`] — Usenet, JSON-RPC positional params + HTTP Basic.
//!
//! # Design: one narrow HTTP seam, richer-than-core status
//!
//! Every adapter routes its I/O through [`HttpTransport`] so contract tests can
//! replay recorded API exchanges with no live client (the integration-test rule
//! in `docs/06-integrations.md`). Production uses [`ReqwestTransport`].
//!
//! `cellarr-core`'s frozen [`cellarr_core::DownloadStatus`] is a four-state
//! summary, which is all the pipeline branches on. Adapters additionally expose
//! a [`progress`](QbittorrentClient::progress)-style method returning
//! [`DownloadProgress`] (on-disk path for Import, seed ratio/time for gated
//! removal) — detail core deliberately does not carry. See the crate report's
//! `coreGaps`.
//!
//! # Category scoping
//!
//! Every `add` tags the download with cellarr's category so cellarr only ever
//! touches its own downloads; [`DownloadProgress::is_in_category`] lets callers
//! refuse to act on a foreign download.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod http;
pub mod lifecycle;
pub mod nzbget;
pub mod qbittorrent;
pub mod sabnzbd;

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
        QbittorrentClient::status(self, download_id).await
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
        SabnzbdClient::status(self, download_id).await
    }

    async fn remove(&self, download_id: &str, delete_data: bool) -> Result<(), Self::Error> {
        SabnzbdClient::remove(self, download_id, delete_data).await
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
        NzbgetClient::status(self, download_id).await
    }

    async fn remove(&self, download_id: &str, delete_data: bool) -> Result<(), Self::Error> {
        NzbgetClient::remove(self, download_id, delete_data).await
    }
}
