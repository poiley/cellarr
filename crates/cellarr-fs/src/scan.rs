//! Library scan / inventory.
//!
//! [`scan`] walks a root folder and reports the files it finds, so the pipeline
//! can "recognize in place" during migration (adopt an existing library without
//! moving anything) and refresh its picture of disk on demand. Scanning is a
//! pure read: it never mutates the filesystem.
//!
//! The walk is iterative (no recursion, so a pathological tree cannot overflow
//! the stack), skips hidden entries and obvious non-media debris, and records
//! enough per file (size, whether it is a hardlink) for the planner and the
//! decision engine to work without a second stat.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use cellarr_core::{MediaFile, MediaFileId, Quality};

use crate::error::{FsError, Result};

/// One discovered file in a library scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryEntry {
    /// Absolute path to the file.
    pub path: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// Number of hardlinks to this file's inode. `>1` means another path (e.g. a
    /// torrent client's download dir) shares the data — relevant to whether a
    /// "delete" actually frees space.
    pub link_count: u64,
}

impl InventoryEntry {
    /// Adopt this discovered file as a [`MediaFile`] record, given an identifier
    /// and the [`Quality`] resolved for it (by parsing the name upstream).
    ///
    /// This is the "recognize in place" bridge: migration walks an existing
    /// library with [`scan`] and turns each entry into a persisted `media_file`
    /// without moving a byte. The `quality` is the same core vocabulary the
    /// decision engine ranks, so an adopted file is immediately comparable to a
    /// newly imported one. `languages`, `media_info`, and `custom_format_score`
    /// are left empty/`None` until a deeper probe/score pass fills them in.
    #[must_use]
    pub fn as_media_file(&self, id: MediaFileId, quality: Quality) -> MediaFile {
        MediaFile {
            id,
            path: self.path.to_string_lossy().into_owned(),
            size: self.size,
            quality,
            languages: Vec::new(),
            media_info: None,
            custom_format_score: None,
            // A scanned-in-place file has no grab provenance, so its release type
            // is unknown until a deeper identify/reconcile pass attributes it.
            release_type: None,
        }
    }
}

/// The result of scanning a library root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    /// Every media-candidate file found, in deterministic (sorted) order so
    /// scans are reproducible and diffable.
    pub entries: Vec<InventoryEntry>,
}

/// Walk `root` and inventory the files beneath it.
///
/// Dispatched to a blocking thread; the walk itself is synchronous I/O.
///
/// # Errors
/// - [`FsError::MissingPath`] if `root` does not exist.
/// - [`FsError::Io`] if a directory cannot be read or a file cannot be stat'd.
pub async fn scan(root: impl Into<PathBuf>) -> Result<Inventory> {
    let root = root.into();
    match tokio::task::spawn_blocking(move || scan_blocking(&root)).await {
        Ok(r) => r,
        Err(e) => Err(FsError::TaskJoin(e.to_string())),
    }
}

fn scan_blocking(root: &Path) -> Result<Inventory> {
    scan_blocking_filtered(root, &|_| true, None)
}

/// Walk `root`, keeping only files `keep` accepts, and stop once `limit` are kept
/// (`None` = unbounded). Dispatched to a blocking thread.
///
/// This bounds BOTH the result size AND the walk cost: `keep` is checked before the
/// file is `stat`-ed, and the walk stops descending the moment it has `limit`
/// matches — so a caller wanting a small preview of a huge library (the manual
/// import auto-surface) never `stat`s the whole tree. `keep` runs on the blocking
/// thread, so it must be cheap and non-blocking (a path/extension test, a set
/// membership check).
///
/// # Errors
/// Same as [`scan`].
pub async fn scan_filtered<F>(
    root: impl Into<PathBuf>,
    keep: F,
    limit: Option<usize>,
) -> Result<Inventory>
where
    F: Fn(&Path) -> bool + Send + 'static,
{
    let root = root.into();
    match tokio::task::spawn_blocking(move || scan_blocking_filtered(&root, &keep, limit)).await {
        Ok(r) => r,
        Err(e) => Err(FsError::TaskJoin(e.to_string())),
    }
}

fn scan_blocking_filtered<F: Fn(&Path) -> bool>(
    root: &Path,
    keep: &F,
    limit: Option<usize>,
) -> Result<Inventory> {
    if !root.exists() {
        return Err(FsError::MissingPath {
            path: root.to_path_buf(),
        });
    }

    let mut entries = Vec::new();
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    queue.push_back(root.to_path_buf());

    'walk: while let Some(dir) = queue.pop_front() {
        let read = fs::read_dir(&dir).map_err(|e| FsError::io(&dir, e))?;
        for item in read {
            let item = item.map_err(|e| FsError::io(&dir, e))?;
            let path = item.path();
            if is_hidden(&path) {
                continue;
            }
            let file_type = item.file_type().map_err(|e| FsError::io(&path, e))?;
            if file_type.is_dir() {
                queue.push_back(path);
            } else if file_type.is_file() && keep(&path) {
                // `keep` gates the (relatively costly) stat, so a filtered-out file
                // is never stat-ed.
                let meta = item.metadata().map_err(|e| FsError::io(&path, e))?;
                entries.push(InventoryEntry {
                    path,
                    size: meta.len(),
                    link_count: link_count(&meta),
                });
                if limit.is_some_and(|n| entries.len() >= n) {
                    break 'walk;
                }
            }
            // Symlinks and special files are intentionally ignored.
        }
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Inventory { entries })
}

/// An **incremental** filtered scan: like [`scan_filtered`] but skips `read_dir`
/// for any directory whose modification time is unchanged since the last scan
/// (passed in `known_dirs` as `path → unix-mtime-secs`), reconstructing that
/// directory's subtree from the recorded set instead of listing it from disk.
///
/// A directory's mtime changes when a direct child is added, removed, or renamed —
/// so an unchanged directory has no new files, and only new/changed directories
/// are read from disk. On a large tree that barely changes between runs this turns
/// a full walk (a `read_dir` per directory over the network) into a set of cheap
/// `stat`s plus a handful of `read_dir`s.
///
/// Returns the files found (new/untracked media under changed directories, per
/// `keep`) plus the CURRENT `path → mtime` map, which the caller persists for the
/// next run. The first run (empty `known_dirs`) reads everything and records the
/// tree; subsequent runs read only what changed. Directories that vanished are
/// simply absent from the returned map.
pub async fn scan_incremental<F>(
    root: impl Into<PathBuf>,
    keep: F,
    known_dirs: std::collections::HashMap<String, i64>,
) -> Result<(Inventory, std::collections::HashMap<String, i64>)>
where
    F: Fn(&Path) -> bool + Send + 'static,
{
    let root = root.into();
    match tokio::task::spawn_blocking(move || {
        scan_incremental_blocking(&root, &keep, &known_dirs)
    })
    .await
    {
        Ok(r) => r,
        Err(e) => Err(FsError::TaskJoin(e.to_string())),
    }
}

fn scan_incremental_blocking<F: Fn(&Path) -> bool>(
    root: &Path,
    keep: &F,
    known: &std::collections::HashMap<String, i64>,
) -> Result<(Inventory, std::collections::HashMap<String, i64>)> {
    use std::collections::HashMap;
    if !root.exists() {
        return Err(FsError::MissingPath {
            path: root.to_path_buf(),
        });
    }
    // Reconstruct the recorded subtree (parent → child dirs) so an UNCHANGED
    // directory can enqueue its subdirectories without a `read_dir`.
    let mut children: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for d in known.keys() {
        if let Some(parent) = Path::new(d).parent() {
            children
                .entry(parent.to_string_lossy().into_owned())
                .or_default()
                .push(PathBuf::from(d));
        }
    }

    let mut entries = Vec::new();
    let mut current: HashMap<String, i64> = HashMap::new();
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    queue.push_back(root.to_path_buf());

    while let Some(dir) = queue.pop_front() {
        let dir_str = dir.to_string_lossy().into_owned();
        let Some(mtime) = dir_mtime_nanos(&dir) else {
            continue; // gone/unreadable since it was recorded — drop it
        };
        current.insert(dir_str.clone(), mtime);

        if known.get(&dir_str) == Some(&mtime) {
            // Unchanged: no direct entries changed → no new files here. Descend
            // into the recorded subdirectories (each is re-checked in turn).
            if let Some(subs) = children.get(&dir_str) {
                queue.extend(subs.iter().cloned());
            }
            continue;
        }

        // New or changed: list it, emit new media files, enqueue real subdirs.
        let read = fs::read_dir(&dir).map_err(|e| FsError::io(&dir, e))?;
        for item in read {
            let item = item.map_err(|e| FsError::io(&dir, e))?;
            let path = item.path();
            if is_hidden(&path) {
                continue;
            }
            let file_type = item.file_type().map_err(|e| FsError::io(&path, e))?;
            if file_type.is_dir() {
                queue.push_back(path);
            } else if file_type.is_file() && keep(&path) {
                let meta = item.metadata().map_err(|e| FsError::io(&path, e))?;
                entries.push(InventoryEntry {
                    path,
                    size: meta.len(),
                    link_count: link_count(&meta),
                });
            }
        }
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((Inventory { entries }, current))
}

/// A directory's modification time as Unix nanoseconds, or `None` if it cannot
/// be read (vanished / permission).
///
/// We use nanosecond precision — not seconds — so a file added in the same wall
/// second as the previous scan still bumps the recorded mtime and is detected on
/// the next pass. On a filesystem that only records second granularity the nanos
/// are zero-padded, so this degrades cleanly to second precision. Nanoseconds
/// since the epoch stay well within `i64` (`~1.8e18 < 9.2e18`) until year 2262.
fn dir_mtime_nanos(dir: &Path) -> Option<i64> {
    let modified = fs::metadata(dir).ok()?.modified().ok()?;
    let nanos = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    i64::try_from(nanos).ok()
}

/// Whether a path's final component begins with a dot. We skip dotfiles and
/// dot-directories (e.g. `.DS_Store`, `.unpack`) — they are never library media.
fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

#[cfg(unix)]
fn link_count(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.nlink()
}

#[cfg(not(unix))]
fn link_count(_meta: &fs::Metadata) -> u64 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scan_missing_root_errors() {
        let err = scan(PathBuf::from("/nonexistent/cellarr/scan/root"))
            .await
            .unwrap_err();
        assert!(matches!(err, FsError::MissingPath { .. }));
    }

    #[tokio::test]
    async fn incremental_scan_reads_only_changed_directories() {
        use std::collections::{HashMap, HashSet};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("A/Season 01")).unwrap();
        std::fs::create_dir_all(root.join("B")).unwrap();
        std::fs::write(root.join("A/Season 01/ep1.mkv"), b"abc").unwrap();
        std::fs::write(root.join("B/movie.mkv"), b"defg").unwrap();

        // First run (no known dirs): reads everything, finds both files, records
        // the directory tree.
        let keep = |p: &std::path::Path| {
            p.extension().and_then(|e| e.to_str()) == Some("mkv")
        };
        let (inv1, dirs1) = scan_incremental(root.to_path_buf(), keep, HashMap::new())
            .await
            .unwrap();
        assert_eq!(inv1.entries.len(), 2, "first run finds both files");
        assert!(dirs1.len() >= 4, "root + A + A/Season 01 + B are recorded");

        // Second run with the recorded tree and nothing changed on disk: finds NO
        // files (every directory's mtime is unchanged, so none are re-read).
        let keep2 = |p: &std::path::Path| {
            p.extension().and_then(|e| e.to_str()) == Some("mkv")
        };
        let (inv2, dirs2) = scan_incremental(root.to_path_buf(), keep2, dirs1.clone())
            .await
            .unwrap();
        assert!(
            inv2.entries.is_empty(),
            "an unchanged tree yields no candidates: {:?}",
            inv2.entries
        );
        assert_eq!(
            dirs2.keys().collect::<HashSet<_>>(),
            dirs1.keys().collect::<HashSet<_>>(),
            "the recorded directory set is stable"
        );

        // Add a new file under one directory: only that directory's mtime changed,
        // so only it is re-read. A re-read directory surfaces ALL its files (the
        // caller dedups already-seen ones), but the untouched B/ is skipped
        // entirely — so we see A's two episodes and nothing from B.
        std::fs::write(root.join("A/Season 01/ep2.mkv"), b"hij").unwrap();
        let keep3 = |p: &std::path::Path| {
            p.extension().and_then(|e| e.to_str()) == Some("mkv")
        };
        let (inv3, _dirs3) = scan_incremental(root.to_path_buf(), keep3, dirs2)
            .await
            .unwrap();
        let paths: Vec<_> = inv3.entries.iter().map(|e| e.path.clone()).collect();
        assert_eq!(
            inv3.entries.len(),
            2,
            "the changed directory re-surfaces both its episodes: {paths:?}"
        );
        assert!(paths.iter().any(|p| p.ends_with("ep1.mkv")));
        assert!(paths.iter().any(|p| p.ends_with("ep2.mkv")));
        assert!(
            !paths.iter().any(|p| p.ends_with("movie.mkv")),
            "the untouched B/ directory is not re-read"
        );
    }

    #[tokio::test]
    async fn scan_finds_files_and_skips_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Show/Season 01")).unwrap();
        std::fs::write(root.join("Show/Season 01/ep1.mkv"), b"abc").unwrap();
        std::fs::write(root.join("Show/.DS_Store"), b"x").unwrap();
        std::fs::create_dir_all(root.join(".unpack")).unwrap();
        std::fs::write(root.join(".unpack/partial.mkv"), b"y").unwrap();

        let inv = scan(root).await.unwrap();
        assert_eq!(inv.entries.len(), 1);
        assert!(inv.entries[0].path.ends_with("ep1.mkv"));
        assert_eq!(inv.entries[0].size, 3);
    }

    #[tokio::test]
    async fn entry_adopts_as_media_file_carrying_core_quality() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Movie Bluray-1080p.mkv"), b"abcdef").unwrap();

        let inv = scan(root).await.unwrap();
        let entry = &inv.entries[0];

        let id = MediaFileId::new();
        let quality = Quality::new("Bluray-1080p", 9);
        let mf = entry.as_media_file(id, quality.clone());

        assert_eq!(mf.id, id);
        assert_eq!(mf.size, 6);
        assert!(mf.path.ends_with("Movie Bluray-1080p.mkv"));
        assert_eq!(mf.quality, quality);
        assert!(mf.languages.is_empty());
        assert!(mf.media_info.is_none());
        assert!(mf.custom_format_score.is_none());
    }

    #[tokio::test]
    async fn scan_is_sorted_and_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["c.mkv", "a.mkv", "b.mkv"] {
            std::fs::write(root.join(name), b"z").unwrap();
        }
        let inv = scan(root).await.unwrap();
        let names: Vec<_> = inv
            .entries
            .iter()
            .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.mkv", "b.mkv", "c.mkv"]);
    }

    #[tokio::test]
    async fn scan_filtered_keeps_only_matches_and_honors_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["a.mkv", "b.mkv", "c.mkv", "d.mkv", "notes.txt", "poster.jpg"] {
            std::fs::write(root.join(name), b"z").unwrap();
        }
        // `keep` selects only .mkv — the non-video files are never inventoried.
        let is_mkv = |p: &Path| p.extension().and_then(|e| e.to_str()) == Some("mkv");

        // Unbounded: every .mkv, no .txt/.jpg.
        let all = scan_filtered(root.to_path_buf(), is_mkv, None).await.unwrap();
        assert_eq!(all.entries.len(), 4, "only the four videos are kept");
        assert!(all.entries.iter().all(|e| e.path.extension().unwrap() == "mkv"));

        // Bounded: stops after `limit` matches.
        let two = scan_filtered(root.to_path_buf(), is_mkv, Some(2)).await.unwrap();
        assert_eq!(two.entries.len(), 2, "the limit caps the kept entries");
    }
}
