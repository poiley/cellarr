//! Filesystem health: the loud same-filesystem (hardlink-feasibility) warning.
//!
//! The single biggest silent footgun in a Sonarr/Radarr-style stack is a
//! **downloads directory on a different filesystem than the library**. When the
//! two are on the same filesystem, an import is an instant [`hardlink`] — it
//! costs no extra disk, and it *preserves the seeding copy* the torrent client
//! is still serving. When they are on different filesystems, a hardlink is
//! impossible, so [`hardlink_or_copy`] silently falls back to a full
//! **copy** ([`crate::fsops::copy_durable_blocking`]): every import now doubles
//! the disk used and races seeding against deletion. The originals do this
//! fallback *silently*; users discover it only when a disk fills or seeding
//! breaks.
//!
//! cellarr makes it **loud**: at config time (and on every `/api/v3/health`
//! read) it compares the `st_dev` of the configured downloads directory against
//! every library root and raises a [`FilesystemWarning`] for each root that does
//! not share a device with downloads. This is the deliberate differentiator
//! called out in `docs/parity/REPLACEMENT-ROADMAP.md` (§6).
//!
//! [`hardlink`]: std::fs::hard_link
//! [`hardlink_or_copy`]: crate::hardlink_or_copy

use std::path::{Path, PathBuf};

use crate::error::{FsError, Result};

/// A health warning about the import filesystem layout.
///
/// Today the only variant is the cross-filesystem (silent-copy-fallback) case,
/// but the type is an enum so future filesystem health checks (unwritable root,
/// downloads dir missing) slot in without changing the call sites that render
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesystemWarning {
    /// The downloads directory and a library root are on **different**
    /// filesystems, so imports into that root cannot hardlink and will fall back
    /// to a full copy (double disk use; seeding copy not preserved).
    CrossFilesystem {
        /// The configured downloads directory.
        downloads_dir: PathBuf,
        /// The library root that is on a different filesystem from downloads.
        library_root: PathBuf,
    },
}

impl FilesystemWarning {
    /// A stable machine-readable source key, matching the v3 health record
    /// `source` field convention (`<Subject>Check`).
    #[must_use]
    pub fn source(&self) -> &'static str {
        match self {
            FilesystemWarning::CrossFilesystem { .. } => "ImportMechanismCheck",
        }
    }

    /// A human-readable, deliberately loud message for the health surface and the
    /// log line.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            FilesystemWarning::CrossFilesystem {
                downloads_dir,
                library_root,
            } => format!(
                "Downloads directory ({}) is on a DIFFERENT filesystem from the library root ({}): \
                 imports cannot hardlink and will fall back to a full copy — doubling disk use and \
                 not preserving the seeding copy. Put downloads and the library on the same \
                 filesystem to enable instant hardlink imports.",
                downloads_dir.display(),
                library_root.display(),
            ),
        }
    }
}

/// Compare the filesystem (`st_dev`) of `downloads_dir` against every entry in
/// `library_roots`, returning a [`FilesystemWarning::CrossFilesystem`] for each
/// root that does **not** share a device with downloads.
///
/// This is the engine behind the loud health warning. It is pure (no mutation)
/// and synchronous: callers run it from the health endpoint and at config time.
///
/// Paths that do not exist are resolved to their nearest existing ancestor
/// before `stat` (a library root may be configured before it is created); if
/// neither `downloads_dir` nor a root has any existing ancestor to stat, that
/// pair is skipped rather than erroring — we never want a missing optional path
/// to take down the whole health read.
///
/// # Errors
/// [`FsError::Io`] only if an existing path that we resolved cannot be `stat`'d
/// for a reason other than non-existence (e.g. a permission error reading the
/// downloads directory) — a genuine fault worth surfacing.
pub fn check_same_filesystem(
    downloads_dir: &Path,
    library_roots: &[PathBuf],
) -> Result<Vec<FilesystemWarning>> {
    // Resolve downloads to a stat-able path; if it has no existing ancestor at
    // all we cannot judge same-vs-different, so we emit nothing (a separate
    // "downloads dir missing" check is the right place for that signal).
    let Some(dl_probe) = nearest_existing(downloads_dir) else {
        return Ok(Vec::new());
    };
    let dl_dev = device_of(&dl_probe)?;

    let mut roots_with_dev = Vec::with_capacity(library_roots.len());
    for root in library_roots {
        let Some(root_probe) = nearest_existing(root) else {
            continue;
        };
        roots_with_dev.push((root.clone(), device_of(&root_probe)?));
    }

    Ok(warnings_from_devices(
        downloads_dir,
        dl_dev,
        &roots_with_dev,
    ))
}

/// The pure device-comparison core: given the downloads device and each library
/// root paired with its resolved device, emit a [`FilesystemWarning`] for every
/// root whose device differs from downloads.
///
/// Split out from [`check_same_filesystem`] so the cross-filesystem branch can be
/// tested deterministically without provisioning a real second filesystem (which
/// is awkward and privileged on most CI hosts). The real `st_dev` resolution is
/// exercised separately by the same-filesystem tempdir test.
fn warnings_from_devices(
    downloads_dir: &Path,
    downloads_dev: u64,
    roots_with_dev: &[(PathBuf, u64)],
) -> Vec<FilesystemWarning> {
    roots_with_dev
        .iter()
        .filter(|(_, dev)| *dev != downloads_dev)
        .map(|(root, _)| FilesystemWarning::CrossFilesystem {
            downloads_dir: downloads_dir.to_path_buf(),
            library_root: root.clone(),
        })
        .collect()
}

/// The nearest existing path at or above `path` (walking up the tree). Kept here
/// (mirrors the helper in [`crate::fsops`]) so the health check can stat a root
/// that is configured-but-not-yet-created.
fn nearest_existing(path: &Path) -> Option<PathBuf> {
    let mut cur = Some(path);
    while let Some(p) = cur {
        if p.exists() {
            return Some(p.to_path_buf());
        }
        cur = p.parent();
    }
    None
}

#[cfg(unix)]
fn device_of(path: &Path) -> Result<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path)
        .map(|m| m.dev())
        .map_err(|e| FsError::io(path, e))
}

#[cfg(not(unix))]
fn device_of(path: &Path) -> Result<u64> {
    // Without a portable device id we cannot prove two paths share a filesystem.
    // Returning a path-derived sentinel makes every distinct path look like a
    // different device, which would spam false warnings; instead we treat the
    // device as unknowable by hashing the *root component* so paths under the
    // same drive letter/share compare equal. Correctness of imports does not
    // depend on this (the copy path is always safe); only the warning's
    // precision does.
    use std::hash::{Hash, Hasher};
    std::fs::metadata(path).map_err(|e| FsError::io(path, e))?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let root: PathBuf = path.components().take(1).collect();
    root.hash(&mut h);
    Ok(h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn same_filesystem_emits_no_warning() {
        // downloads and library are both subdirs of one tempdir → one filesystem.
        let dir = tmpdir();
        let downloads = dir.path().join("downloads");
        let library = dir.path().join("library/movies");
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::create_dir_all(&library).unwrap();

        let warnings = check_same_filesystem(&downloads, &[library]).unwrap();
        assert!(
            warnings.is_empty(),
            "same-filesystem layout must not warn, got {warnings:?}"
        );
    }

    #[test]
    fn library_root_configured_but_not_yet_created_resolves_to_ancestor() {
        let dir = tmpdir();
        let downloads = dir.path().join("downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        // Library root does not exist yet, but its parent (the tempdir) does and
        // is on the same device as downloads → still no warning.
        let library = dir.path().join("library/not/created/yet");

        let warnings = check_same_filesystem(&downloads, &[library]).unwrap();
        assert!(warnings.is_empty());
    }

    #[test]
    fn warning_message_is_loud_and_names_both_paths() {
        let w = FilesystemWarning::CrossFilesystem {
            downloads_dir: PathBuf::from("/downloads"),
            library_root: PathBuf::from("/library/movies"),
        };
        let msg = w.message();
        assert!(msg.contains("/downloads"));
        assert!(msg.contains("/library/movies"));
        assert!(msg.contains("DIFFERENT filesystem"));
        assert!(msg.to_lowercase().contains("copy"));
        assert_eq!(w.source(), "ImportMechanismCheck");
    }

    #[test]
    fn cross_filesystem_raises_a_warning_per_off_device_root() {
        // Deterministic exercise of the cross-fs branch: downloads on device 10,
        // one library root on the SAME device (no warning) and one on a
        // DIFFERENT device (warning). This proves the st_dev comparison actually
        // triggers — no real second filesystem required.
        let downloads = PathBuf::from("/mnt/downloads");
        let same_dev_root = PathBuf::from("/mnt/library/tv");
        let other_dev_root = PathBuf::from("/other/library/movies");

        let warnings = warnings_from_devices(
            &downloads,
            10,
            &[(same_dev_root.clone(), 10), (other_dev_root.clone(), 42)],
        );

        assert_eq!(warnings.len(), 1, "exactly the off-device root must warn");
        assert_eq!(
            warnings[0],
            FilesystemWarning::CrossFilesystem {
                downloads_dir: downloads,
                library_root: other_dev_root,
            }
        );
    }

    #[test]
    fn missing_downloads_dir_with_no_ancestor_emits_nothing() {
        // A path whose every ancestor is absent cannot be judged; we emit no
        // warning rather than erroring (so a transient config never breaks the
        // whole health read).
        let downloads = PathBuf::from("/this/path/should/not/exist/cellarr-xyz");
        let library = std::env::temp_dir();
        // Only meaningful if the bogus path truly has no existing ancestor; on
        // unix "/" exists so nearest_existing("/this/...") resolves to "/". That
        // means we DO get a dev for it. Use a relative path with no parent to
        // force the no-ancestor branch deterministically.
        let _ = (&downloads, &library);
        let no_ancestor = PathBuf::from("definitely-not-a-real-relative-dir-cellarr");
        let warnings = check_same_filesystem(&no_ancestor, &[std::env::temp_dir()]).unwrap();
        assert!(warnings.is_empty());
    }
}
