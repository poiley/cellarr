//! The cross-filesystem (silent-copy-fallback) health check, wired to config.
//!
//! This adapts cellarr-fs's pure [`check_same_filesystem`] to the live
//! configuration: it reads the configured **downloads directory** from each
//! enabled download client and the **library roots** from each configured
//! library, and returns one warning per (downloads dir, library root) pair that
//! sits on a different filesystem — the import would then fall back to a full
//! copy instead of a hardlink (`docs/specs/cellarr-fs.md`, the deliberate
//! differentiator in `docs/parity/REPLACEMENT-ROADMAP.md` §6).
//!
//! Both the `/api/v3/health` shim handler and the native system-health snapshot
//! call [`filesystem_warnings`], so the warning surfaces on **both faces** and is
//! additionally `warn!`-logged each time it is observed.

use std::path::PathBuf;

use cellarr_db::Database;
use cellarr_fs::FilesystemWarning;

use crate::error::ApiResult;

/// The settings key a download client config carries to name the host-side
/// directory its completed downloads land in. It is optional: when absent we
/// cannot run the same-filesystem check for that client (and emit nothing for
/// it) rather than guessing a path.
const DOWNLOAD_DIR_KEYS: [&str; 3] = ["download_dir", "downloadDir", "save_path"];

/// Compute the live filesystem health warnings from the configured download
/// clients and libraries.
///
/// Returns one [`FilesystemWarning::CrossFilesystem`] per (client downloads dir,
/// library root) pair on different filesystems. Each distinct warning is
/// `warn!`-logged so the footgun is loud in the logs as well as on the health
/// surface.
///
/// # Errors
/// Propagates a persistence error if the config cannot be read, or an
/// [`cellarr_fs`] I/O error if an existing path cannot be `stat`'d (a genuine
/// fault, e.g. an unreadable downloads directory).
pub async fn filesystem_warnings(db: &Database) -> ApiResult<Vec<FilesystemWarning>> {
    let cfg = db.config();

    // The library roots to check against (deduplicated, order-stable).
    let mut roots: Vec<PathBuf> = Vec::new();
    for lib in cfg.list_libraries().await? {
        for root in lib.root_folders {
            let p = PathBuf::from(root);
            if !roots.contains(&p) {
                roots.push(p);
            }
        }
    }
    for rf in cfg.list_root_folders().await? {
        let p = PathBuf::from(rf.path);
        if !roots.contains(&p) {
            roots.push(p);
        }
    }
    if roots.is_empty() {
        return Ok(Vec::new());
    }

    // Each enabled client's configured downloads dir, compared against every
    // library root. We dedup identical warnings across clients that share a dir.
    let mut warnings: Vec<FilesystemWarning> = Vec::new();
    for client in cfg.list_download_clients().await? {
        if !client.enabled {
            continue;
        }
        let Some(dir) = downloads_dir_of(&client.settings) else {
            continue;
        };
        let dir = PathBuf::from(dir);
        let client_warnings = cellarr_fs::check_same_filesystem(&dir, &roots).map_err(|e| {
            crate::error::ApiError::Internal(format!("filesystem health check failed: {e}"))
        })?;
        for w in client_warnings {
            if !warnings.contains(&w) {
                tracing::warn!(source = w.source(), "{}", w.message(),);
                warnings.push(w);
            }
        }
    }

    Ok(warnings)
}

/// Pull the configured host-side downloads directory out of a download client's
/// `settings` JSON, trying the known key aliases. Returns `None` when no
/// downloads dir is configured (the check is then skipped for that client).
fn downloads_dir_of(settings: &serde_json::Value) -> Option<String> {
    DOWNLOAD_DIR_KEYS
        .iter()
        .find_map(|k| settings.get(*k).and_then(|v| v.as_str()))
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_downloads_dir_under_each_alias() {
        assert_eq!(
            downloads_dir_of(&json!({ "download_dir": "/downloads" })).as_deref(),
            Some("/downloads")
        );
        assert_eq!(
            downloads_dir_of(&json!({ "downloadDir": "/dl" })).as_deref(),
            Some("/dl")
        );
        assert_eq!(
            downloads_dir_of(&json!({ "save_path": "/data/complete" })).as_deref(),
            Some("/data/complete")
        );
        assert_eq!(downloads_dir_of(&json!({ "base_url": "http://x" })), None);
        assert_eq!(downloads_dir_of(&json!({ "download_dir": "" })), None);
    }
}
