//! The cross-crate seam traits.
//!
//! These are the contracts other crates implement: each media type implements
//! [`MediaModule`]; each metadata adapter implements [`MetadataSource`]; each
//! indexer and download client implements [`Indexer`] / [`DownloadClient`].
//! Defining them here keeps `cellarr-core` the single vocabulary every crate
//! speaks; the implementations live in their own crates.
//!
//! Async methods use [`async_trait`] so the traits stay object-safe (callers
//! hold them behind `dyn` for runtime configuration). Each trait carries an
//! associated `Error: std::error::Error` so implementations report typed
//! failures without core depending on any I/O crate.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::decision::GrabRequest;
use crate::media::{ContentRef, MediaType};
use crate::parsed::ParsedRelease;
use crate::release::{ContentMatch, Release};

/// Search parameters a media module produces for querying indexers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchTerms {
    /// Title/alias queries to try, most specific first.
    pub queries: Vec<String>,
    /// External IDs to attach to the query (e.g. `tvdbid`, `imdbid`), as
    /// `(key, value)` pairs.
    pub ids: Vec<(String, String)>,
    /// Season/episode (or equivalent) query parameters, as `(key, value)` pairs.
    pub numbering: Vec<(String, String)>,
}

/// Naming tokens a media module exposes for the rename engine.
///
/// The rename engine substitutes these into the user's naming format; the media
/// module is the only thing that knows how to fill them for its type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamingTokens {
    /// Token name → value (e.g. `{ "Series Title": "The Show", "Season": "02" }`).
    pub tokens: Vec<(String, String)>,
}

/// The per-media-type behavior the pipeline delegates to.
///
/// Implemented once per media type in `cellarr-media`. The pipeline never
/// branches on [`MediaType`]; it asks the matching `MediaModule` instead.
#[async_trait]
pub trait MediaModule: Send + Sync {
    /// The typed error this module reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// The media type this module serves.
    fn media_type(&self) -> MediaType;

    /// Produce indexer search terms for a content node.
    async fn search_terms(&self, content: &ContentRef) -> Result<SearchTerms, Self::Error>;

    /// Given a parsed release, determine which content node(s) it satisfies and
    /// with what confidence.
    async fn match_release(&self, parsed: &ParsedRelease)
        -> Result<Vec<ContentMatch>, Self::Error>;

    /// Produce the naming tokens the rename engine needs for a content node.
    async fn naming_tokens(&self, content: &ContentRef) -> Result<NamingTokens, Self::Error>;
}

/// A metadata adapter (TMDb, TheTVDB, MusicBrainz, OpenLibrary, AniDB).
///
/// Returns opaque JSON for typed payloads so core stays free of every provider's
/// schema; the per-type metadata crates deserialize into their own structs.
#[async_trait]
pub trait MetadataSource: Send + Sync {
    /// The typed error this source reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// The media type this source provides metadata for.
    fn media_type(&self) -> MediaType;

    /// Search for candidate identities by free-text query.
    async fn search(&self, query: &str) -> Result<Vec<serde_json::Value>, Self::Error>;

    /// Fetch the full metadata payload for an external id.
    async fn fetch(&self, external_id: &str) -> Result<serde_json::Value, Self::Error>;

    /// Fetch scene/numbering mappings for a series (used to reconcile anime
    /// absolute numbering at Identify). Empty when not applicable.
    async fn scene_mapping(
        &self,
        _external_id: &str,
    ) -> Result<Vec<serde_json::Value>, Self::Error> {
        Ok(Vec::new())
    }
}

/// An indexer integration (Torznab, Newznab, Cardigann).
#[async_trait]
pub trait Indexer: Send + Sync {
    /// The typed error this indexer reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// A human-facing name for logs and the UI.
    fn name(&self) -> &str;

    /// Run a search and return raw candidate releases.
    async fn search(&self, terms: &SearchTerms) -> Result<Vec<Release>, Self::Error>;

    /// Fetch the latest releases (RSS-style) for periodic discovery.
    async fn latest(&self) -> Result<Vec<Release>, Self::Error>;
}

/// The coarse lifecycle state of a tracked download.
///
/// This is the four-state summary the pipeline's state machine branches on. The
/// surrounding [`DownloadStatus`] carries the detail (path, progress, seed
/// signals) that Import and torrent-cleanup also need.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    /// Queued but not started.
    Queued,
    /// Actively downloading.
    Downloading,
    /// Finished and ready to import.
    Completed,
    /// Failed (the caller should blocklist and re-search).
    Failed,
}

/// The status of a tracked download, with the detail the executor needs.
///
/// The pipeline branches on [`DownloadStatus::state`]; Import and torrent-cleanup
/// need more than that. Import has to know **where on disk** the finished content
/// landed ([`content_path`](DownloadStatus::content_path)), and ratio/time-gated
/// removal needs the seed signals ([`ratio`](DownloadStatus::ratio),
/// [`seeding_time_secs`](DownloadStatus::seeding_time_secs)). Carrying them on the
/// trait's return type lets both work off the [`DownloadClient`] trait alone,
/// without downcasting to a concrete client.
///
/// `content_path` is `None` until the download is [`DownloadState::Completed`];
/// see [`DownloadClient::status`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadStatus {
    /// The coarse lifecycle state the pipeline branches on.
    pub state: DownloadState,
    /// Fraction complete in `[0.0, 1.0]`.
    pub progress: f32,
    /// The on-disk path of the finished content, set once the download is
    /// [`DownloadState::Completed`] and the client reports a final, importable
    /// location. `None` while queued/downloading. **Required for Import.**
    pub content_path: Option<String>,
    /// Seed ratio (uploaded / downloaded) for torrents, when known. `None` for
    /// Usenet, which does not seed. Used to ratio-gate removal.
    pub ratio: Option<f32>,
    /// Seeding time in seconds for torrents, when known. `None` for Usenet. Used
    /// to time-gate removal.
    pub seeding_time_secs: Option<u64>,
    /// Connected peers (seeds + leechers) for torrents, when known. `None` means
    /// the client does not report it (Usenet, or a client that omits it) — which
    /// the stall detector treats as *unknown*, never as zero. A reported `Some(0)`
    /// is a torrent with no one to download from: a stall signal when paired with
    /// stagnant [`progress`](DownloadStatus::progress).
    #[serde(default)]
    pub peers: Option<u32>,
    /// The client's terminal error text, when it reports one (qBittorrent's
    /// `error`/`missingFiles`, SABnzbd's `Failed` status line). Surfaced on a
    /// [`DownloadState::Failed`] so the failure detail — and the blocklist reason
    /// — names *why* the download died rather than a bare "download failed".
    #[serde(default)]
    pub error_string: Option<String>,
}

impl DownloadStatus {
    /// A status carrying only its coarse state, with no detail. Useful for tests
    /// and for clients that expose nothing beyond the lifecycle state.
    #[must_use]
    pub const fn from_state(state: DownloadState) -> Self {
        Self {
            state,
            progress: 0.0,
            content_path: None,
            ratio: None,
            seeding_time_secs: None,
            peers: None,
            error_string: None,
        }
    }

    /// Whether the download has finished and is ready to import.
    #[must_use]
    pub const fn is_completed(&self) -> bool {
        matches!(self.state, DownloadState::Completed)
    }
}

/// A download-client integration (qBittorrent, Deluge, Transmission, SABnzbd,
/// NZBGet).
#[async_trait]
pub trait DownloadClient: Send + Sync {
    /// The typed error this client reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// A human-facing name for logs and the UI.
    fn name(&self) -> &str;

    /// Hand a grab to the client; returns the client's download id.
    async fn add(&self, grab: &GrabRequest) -> Result<String, Self::Error>;

    /// Poll the status of a download by its client id.
    ///
    /// The returned [`DownloadStatus::content_path`] is `None` until the download
    /// reaches [`DownloadState::Completed`], at which point it is set to the final
    /// on-disk location Import reads from. For torrents the status also carries the
    /// seed [`ratio`](DownloadStatus::ratio) /
    /// [`seeding_time_secs`](DownloadStatus::seeding_time_secs) so removal can be
    /// ratio/time-gated.
    async fn status(&self, download_id: &str) -> Result<DownloadStatus, Self::Error>;

    /// Remove a download (optionally deleting its data).
    async fn remove(&self, download_id: &str, delete_data: bool) -> Result<(), Self::Error>;
}
