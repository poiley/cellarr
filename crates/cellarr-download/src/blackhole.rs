//! Blackhole / watch-folder adapter — the *universal* download client.
//!
//! # Why this works with **any** download client
//!
//! Every other adapter in this crate (qBittorrent, SABnzbd, NZBGet) speaks a
//! client-specific HTTP API. The blackhole adapter speaks **no client API at
//! all**: it is the lowest common denominator every torrent client and Usenet
//! tool already supports — a *watched folder*. cellarr drops the release's
//! `.torrent`/`.nzb` (or a `.magnet` file) into a configured **watch directory**;
//! the user's own client — whatever it is, including ones cellarr has no adapter
//! for — is pointed at that same directory, picks the job up, downloads it, and
//! drops the finished content into a configured **completed directory**. cellarr
//! detects completion by watching that directory. No login, no API version
//! quirks, no per-client code. That is the entire point: **this single adapter is
//! a working download client for every client that can watch a folder.**
//!
//! Because there is no API to query, status is *filesystem-derived*: the job is
//! `Downloading` until a matching output appears in the completed directory, then
//! `Completed` with `content_path` pointing at that output. The handoff to Import
//! is identical to every other adapter — Track reads `content_path` and imports
//! it — so the jobs runner needs no special-casing.
//!
//! # Deterministic ids
//!
//! `add` returns a deterministic download id derived from the dropped file's stem
//! (the release title, sanitized). `status`/`remove` recompute the same stem from
//! the id, so the three calls agree without any persisted handle: the id *is* the
//! file name. The completed item is matched by that same stem, so a client that
//! preserves the job name (torrent name / nzb name) lands its output where
//! `status` looks for it.

use std::path::{Path, PathBuf};

use cellarr_core::{DownloadState, GrabRequest, Protocol};
use serde::Deserialize;

use crate::error::DownloadError;
use crate::http::{HttpRequest, HttpTransport};
use crate::lifecycle::DownloadProgress;

/// Connection-free settings for a blackhole client, deserialized from a
/// [`cellarr_core::DownloadClientConfig`]'s `settings` JSON.
///
/// Both directories are absolute paths cellarr and the user's client can both
/// see (after any [remote-path mapping](crate) the user has configured). The
/// watch directory is where cellarr drops jobs; the completed directory is where
/// the client drops finished content.
#[derive(Debug, Clone, Deserialize)]
pub struct BlackholeSettings {
    /// Directory cellarr writes `.torrent`/`.nzb`/`.magnet` jobs into for the
    /// client to pick up. Sonarr/Radarr call this `torrentFolder`/`nzbFolder`;
    /// we use one neutral name.
    pub watch_folder: String,
    /// Directory the client drops finished content into; `status` watches it for
    /// the matching output.
    pub completed_folder: String,
}

/// A blackhole / watch-folder download client.
///
/// Carries an [`HttpTransport`] only to fetch `.torrent`/`.nzb` bytes from a
/// grab's URL; magnet links are written verbatim with no network call. Tests
/// inject a replay transport (or never trigger a fetch by using magnets) so the
/// adapter stays hermetic.
pub struct BlackholeClient {
    name: String,
    settings: BlackholeSettings,
    category: String,
    /// The protocol this blackhole serves, deciding the dropped file extension
    /// (`.torrent`/`.magnet` for torrents, `.nzb` for Usenet).
    protocol: Protocol,
    transport: Box<dyn HttpTransport>,
}

impl BlackholeClient {
    /// Build a client over the production HTTP transport.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        settings: BlackholeSettings,
        category: impl Into<String>,
        protocol: Protocol,
    ) -> Self {
        Self::with_transport(
            name,
            settings,
            category,
            protocol,
            Box::new(crate::http::ReqwestTransport::new()),
        )
    }

    /// Build a client over a caller-supplied transport (the test seam). The
    /// transport is only consulted when fetching `.torrent`/`.nzb` bytes from an
    /// `http(s)` URL; magnet grabs never touch it.
    #[must_use]
    pub fn with_transport(
        name: impl Into<String>,
        settings: BlackholeSettings,
        category: impl Into<String>,
        protocol: Protocol,
        transport: Box<dyn HttpTransport>,
    ) -> Self {
        Self {
            name: name.into(),
            settings,
            category: category.into(),
            protocol,
            transport,
        }
    }

    /// A human-facing name for logs and the UI.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The category cellarr files its downloads under. The blackhole has no
    /// client-side label, so this is advisory (carried for parity with the other
    /// adapters); scoping is enforced by the dedicated watch/completed dirs.
    #[must_use]
    pub fn category(&self) -> &str {
        &self.category
    }

    fn watch_dir(&self) -> &Path {
        Path::new(&self.settings.watch_folder)
    }

    fn completed_dir(&self) -> &Path {
        Path::new(&self.settings.completed_folder)
    }

    /// Drop the grab into the watch directory for the client to pick up.
    ///
    /// A magnet URL is written verbatim into `<stem>.magnet`; an `http(s)` URL is
    /// fetched (via the transport) and its bytes written into `<stem>.torrent` or
    /// `<stem>.nzb`. Returns the deterministic download id (the file stem), which
    /// `status`/`remove` recompute to find the job again.
    ///
    /// # Errors
    /// [`DownloadError::Config`] if the watch directory cannot be created/written,
    /// [`DownloadError::Transport`]/[`DownloadError::Api`] if a `.torrent`/`.nzb`
    /// fetch fails.
    pub async fn add(&self, grab: &GrabRequest) -> Result<String, DownloadError> {
        let stem = sanitize_stem(&grab.release.title);
        let url = grab.release.download_url.trim();

        std::fs::create_dir_all(self.watch_dir()).map_err(|e| {
            DownloadError::Config(format!(
                "cannot create watch folder {}: {e}",
                self.settings.watch_folder
            ))
        })?;

        let (ext, bytes): (&str, Vec<u8>) = if url.starts_with("magnet:") {
            // A magnet needs no fetch — write it as-is for the client to read.
            ("magnet", url.as_bytes().to_vec())
        } else {
            // A .torrent / .nzb URL: fetch the file bytes through the transport.
            let ext = match self.protocol {
                Protocol::Torrent => "torrent",
                Protocol::Usenet => "nzb",
            };
            let body = self.fetch(url).await?;
            (ext, body)
        };

        let path = self.watch_dir().join(format!("{stem}.{ext}"));
        std::fs::write(&path, &bytes).map_err(|e| {
            DownloadError::Config(format!("cannot write job to {}: {e}", path.display()))
        })?;
        Ok(stem)
    }

    /// Derive the filesystem-observed status of a job.
    ///
    /// Looks for a completed item matching `download_id` in the completed
    /// directory. While none exists the job is `Downloading` (the client is still
    /// working, or never picked it up); once one appears it is `Completed` with
    /// `content_path` set to that item — exactly what Import reads. If neither the
    /// completed item nor the original watch artifact exists, the job is unknown
    /// ([`DownloadError::NotFound`]).
    ///
    /// # Errors
    /// [`DownloadError::NotFound`] when the id matches no watch or completed
    /// artifact; [`DownloadError::Config`] on a directory read failure.
    pub async fn progress(&self, download_id: &str) -> Result<DownloadProgress, DownloadError> {
        if let Some(content) = self.find_completed(download_id)? {
            return Ok(DownloadProgress {
                state: DownloadState::Completed,
                progress: 1.0,
                content_path: Some(content.to_string_lossy().into_owned()),
                // The blackhole cannot observe seeding (no client API); removal of
                // a blackhole job is unconditional.
                ratio: None,
                seeding_time_secs: None,
                category: Some(self.category.clone()),
            });
        }
        if self.watch_artifact(download_id).is_some() {
            // The job is still in the watch dir (or in flight): downloading.
            return Ok(DownloadProgress {
                state: DownloadState::Downloading,
                progress: 0.0,
                content_path: None,
                ratio: None,
                seeding_time_secs: None,
                category: Some(self.category.clone()),
            });
        }
        Err(DownloadError::NotFound(download_id.to_string()))
    }

    /// Remove a job's artifacts. Always removes the watch artifact (the queued
    /// job); when `delete_data` is set, also removes the completed output.
    ///
    /// Idempotent: a missing artifact is not an error (the client may have
    /// consumed the watch file already, or the user may have cleaned up).
    ///
    /// # Errors
    /// [`DownloadError::Config`] only on an unexpected I/O failure removing a file
    /// that does exist.
    pub async fn remove(&self, download_id: &str, delete_data: bool) -> Result<(), DownloadError> {
        if let Some(artifact) = self.watch_artifact(download_id) {
            remove_path(&artifact)?;
        }
        if delete_data {
            if let Some(content) = self.find_completed(download_id)? {
                remove_path(&content)?;
            }
        }
        Ok(())
    }

    /// Fetch `.torrent`/`.nzb` bytes from an `http(s)` URL through the transport.
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, DownloadError> {
        let resp = self.transport.send(HttpRequest::new("GET", url)).await?;
        if !resp.is_success() {
            return Err(DownloadError::Api(format!(
                "fetching job file failed (status {})",
                resp.status
            )));
        }
        Ok(resp.body.into_bytes())
    }

    /// The watch-dir artifact for an id, if one is present (any extension).
    fn watch_artifact(&self, download_id: &str) -> Option<PathBuf> {
        for ext in ["torrent", "nzb", "magnet"] {
            let p = self.watch_dir().join(format!("{download_id}.{ext}"));
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    /// Find the completed item whose name matches `download_id`. The client
    /// preserves the job name, so a finished single file `<stem>.<ext>` or a
    /// finished folder `<stem>/` both match by stem.
    fn find_completed(&self, download_id: &str) -> Result<Option<PathBuf>, DownloadError> {
        let dir = self.completed_dir();
        if !dir.exists() {
            return Ok(None);
        }
        let entries = std::fs::read_dir(dir).map_err(|e| {
            DownloadError::Config(format!(
                "cannot read completed folder {}: {e}",
                self.settings.completed_folder
            ))
        })?;
        for entry in entries {
            let entry = entry
                .map_err(|e| DownloadError::Config(format!("reading completed entry: {e}")))?;
            let path = entry.path();
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            // A finished folder has no extension; its whole name is the stem.
            let folder_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if stem == download_id || folder_name == download_id {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }
}

/// Remove a file or directory at `path`, treating "already gone" as success.
fn remove_path(path: &Path) -> Result<(), DownloadError> {
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(DownloadError::Config(format!(
            "cannot remove {}: {e}",
            path.display()
        ))),
    }
}

/// Sanitize a release title into a filesystem-safe, deterministic stem used as
/// both the dropped file name and the download id. Path separators and other
/// awkward characters become `_`; the result is stable for a given title so
/// `add`/`status`/`remove` agree.
fn sanitize_stem(title: &str) -> String {
    let mapped: String = title
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if mapped.is_empty() {
        "download".to_string()
    } else {
        mapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_separators() {
        assert_eq!(sanitize_stem("Show/S01E01"), "Show_S01E01");
        assert_eq!(sanitize_stem("  Movie 2024  "), "Movie 2024");
        assert_eq!(sanitize_stem(""), "download");
    }
}
