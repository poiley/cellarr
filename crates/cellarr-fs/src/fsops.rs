//! The durable, crash-safe primitives every import is built on.
//!
//! This module is the *only* place that touches the filesystem in a mutating
//! way, and it is written to one rule above all others:
//!
//! > **A new file is fully in place and durable before any old file is removed,
//! > and a failed move never leaves a partial file at the destination.**
//!
//! [`hardlink_or_copy`] prefers a hardlink within a filesystem (instant, and it
//! preserves the seeding copy that a torrent client is still serving). When the
//! source and destination are on different filesystems — where hardlinks are
//! impossible — it copies to a *temporary* file beside the destination, fsyncs
//! the data and the directory, then performs a single atomic `rename` into the
//! final name. A crash before the rename leaves only the temp file (which is
//! cleaned up or ignored); the destination is never observed half-written.
//!
//! All blocking I/O is dispatched to [`tokio::task::spawn_blocking`] so the async
//! reactor is never stalled.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{FsError, Result};

/// How a file was placed at its destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkOutcome {
    /// A hardlink was created; source and destination share inode and data.
    Hardlinked,
    /// The file was copied durably (source and destination are independent).
    Copied,
}

/// Place `src` at `dst`, hardlinking when possible and otherwise copying
/// durably.
///
/// The destination's parent directory must already exist (the import committer
/// creates directory trees as part of its plan). The destination itself must
/// **not** already exist — callers that intend to replace a file move the
/// replacement into place under a temporary name and swap, so we refuse to
/// clobber here as a safety backstop.
///
/// # Errors
/// - [`FsError::MissingPath`] if `src` does not exist.
/// - [`FsError::UnexpectedDestination`] if `dst` already exists.
/// - [`FsError::Io`] for any underlying filesystem failure. On the copy path a
///   failure never leaves a file at `dst`; the temporary is removed.
pub async fn hardlink_or_copy(
    src: impl Into<PathBuf>,
    dst: impl Into<PathBuf>,
) -> Result<LinkOutcome> {
    let src = src.into();
    let dst = dst.into();
    spawn_blocking(move || hardlink_or_copy_blocking(&src, &dst)).await
}

/// The blocking implementation, kept separate so tests can call it directly and
/// so the async wrapper is a thin shim.
fn hardlink_or_copy_blocking(src: &Path, dst: &Path) -> Result<LinkOutcome> {
    if !src.exists() {
        return Err(FsError::MissingPath {
            path: src.to_path_buf(),
        });
    }
    if dst.exists() {
        return Err(FsError::UnexpectedDestination {
            path: dst.to_path_buf(),
        });
    }

    // Prefer a hardlink: instant, and it keeps the seeding copy alive. This
    // succeeds only when src and dst are on the same filesystem.
    match fs::hard_link(src, dst) {
        Ok(()) => return Ok(LinkOutcome::Hardlinked),
        Err(e) if is_cross_device(&e) => { /* fall through to durable copy */ }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            return Err(FsError::UnexpectedDestination {
                path: dst.to_path_buf(),
            });
        }
        // Some filesystems reject hardlinks (e.g. FAT) with EPERM/ENOTSUP even
        // on the same device; treat that as "must copy" rather than a hard
        // failure, matching the originals' resilience.
        Err(e) if is_unsupported(&e) => { /* fall through */ }
        Err(e) => return Err(FsError::io(dst, e)),
    }

    copy_durable_blocking(src, dst)?;
    Ok(LinkOutcome::Copied)
}

/// Copy `src` to `dst` such that `dst` only ever appears atomically and fully
/// durable. Implemented as: copy to a sibling temp file, fsync the file, fsync
/// the parent directory, then `rename` into place.
fn copy_durable_blocking(src: &Path, dst: &Path) -> Result<()> {
    let parent = dst.parent().ok_or_else(|| FsError::Io {
        path: dst.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"),
    })?;

    let tmp = temp_sibling(dst);

    // Guard ensures a partial temp file is removed on any early return, so a
    // failed copy never leaves debris at or near the destination.
    let cleanup = TempCleanup {
        path: Some(tmp.clone()),
    };

    {
        let mut reader = File::open(src).map_err(|e| FsError::io(src, e))?;
        let mut writer = File::create(&tmp).map_err(|e| FsError::io(&tmp, e))?;
        io::copy(&mut reader, &mut writer).map_err(|e| FsError::io(&tmp, e))?;
        // fsync the file contents+metadata before it can be renamed into place.
        writer.sync_all().map_err(|e| FsError::io(&tmp, e))?;
    }

    // The atomic rename: after this returns, dst names the fully-written file.
    fs::rename(&tmp, dst).map_err(|e| FsError::io(dst, e))?;

    // fsync the directory so the rename itself survives a crash. Best-effort on
    // platforms that cannot open a directory for sync.
    fsync_dir(parent)?;

    // The temp no longer exists (renamed); disarm the cleanup guard.
    cleanup.disarm();
    Ok(())
}

/// Remove a file durably: the file is unlinked and its parent directory fsynced
/// so the removal survives a crash. Used at Cleanup to drop replaced files only
/// after their replacements are durable.
///
/// # Errors
/// [`FsError::Io`] if the unlink fails for a reason other than the file already
/// being gone (a missing file is treated as success — removal is idempotent so
/// a resumed cleanup is safe).
pub async fn remove_durable(path: impl Into<PathBuf>) -> Result<()> {
    let path = path.into();
    spawn_blocking(move || remove_durable_blocking(&path)).await
}

fn remove_durable_blocking(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(FsError::io(path, e)),
    }
    if let Some(parent) = path.parent() {
        fsync_dir(parent)?;
    }
    Ok(())
}

/// Create a directory and all parents, returning success if it already exists.
///
/// # Errors
/// [`FsError::Io`] if creation fails.
pub async fn create_dir_all(path: impl Into<PathBuf>) -> Result<()> {
    let path = path.into();
    spawn_blocking(move || fs::create_dir_all(&path).map_err(|e| FsError::io(&path, e))).await
}

/// Whether the destination filesystem (the parent of `dst`) is the same as the
/// source's. Used by the planner to set [`PlannedMove::hardlink`]. A hardlink is
/// only possible within one filesystem.
///
/// [`PlannedMove::hardlink`]: cellarr_core::PlannedMove::hardlink
///
/// # Errors
/// [`FsError::MissingPath`] if neither the destination parent nor any existing
/// ancestor can be stat'd to learn its device.
pub async fn same_filesystem(src: impl Into<PathBuf>, dst: impl Into<PathBuf>) -> Result<bool> {
    let src = src.into();
    let dst = dst.into();
    spawn_blocking(move || same_filesystem_blocking(&src, &dst)).await
}

fn same_filesystem_blocking(src: &Path, dst: &Path) -> Result<bool> {
    let src_dev = device_of(src)?;
    // The destination file may not exist yet; walk up to the nearest existing
    // ancestor (its parent directory) to learn the target device.
    let dst_probe = nearest_existing(dst).ok_or_else(|| FsError::MissingPath {
        path: dst.to_path_buf(),
    })?;
    let dst_dev = device_of(&dst_probe)?;
    Ok(src_dev == dst_dev)
}

/// The nearest existing path at or above `path` (walking up the tree). Used to
/// stat the destination filesystem before the destination file exists.
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

/// Size of a file in bytes, for space checks and verification.
///
/// # Errors
/// [`FsError::Io`] if the file cannot be stat'd.
pub fn file_size(path: &Path) -> Result<u64> {
    fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| FsError::io(path, e))
}

// --- platform helpers -----------------------------------------------------

#[cfg(unix)]
fn device_of(path: &Path) -> Result<u64> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path)
        .map(|m| m.dev())
        .map_err(|e| FsError::io(path, e))
}

#[cfg(not(unix))]
fn device_of(path: &Path) -> Result<u64> {
    // Without a portable device id, fall back to "assume different filesystem"
    // by returning a path-derived sentinel. The copy path is always safe; only
    // the optimization (hardlink) is skipped. Correctness is preserved.
    fs::metadata(path)
        .map(|_| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            path.hash(&mut h);
            h.finish()
        })
        .map_err(|e| FsError::io(path, e))
}

#[cfg(unix)]
fn is_cross_device(e: &io::Error) -> bool {
    // EXDEV
    e.raw_os_error() == Some(18)
}

#[cfg(not(unix))]
fn is_cross_device(_e: &io::Error) -> bool {
    // On non-unix we never attempt a hardlink across volumes meaningfully; any
    // failure routes to the durable copy path, which is always correct.
    true
}

fn is_unsupported(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
    )
}

/// fsync a directory so a rename/unlink within it is durable. Opening a
/// directory for sync is supported on Unix; elsewhere this is a best-effort
/// no-op (the rename is still atomic, just not guaranteed durable across a
/// power loss — acceptable on platforms lacking the primitive).
fn fsync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        match File::open(dir) {
            Ok(f) => f.sync_all().map_err(|e| FsError::io(dir, e)),
            Err(e) => Err(FsError::io(dir, e)),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// A unique temporary sibling name for an in-progress copy. Kept beside the
/// destination so the final `rename` stays within one directory (and thus
/// atomic). The PID and a monotonic counter make concurrent imports collision-
/// free without needing the `tempfile` crate at runtime.
fn temp_sibling(dst: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = dst
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let tmp_name = format!(".cellarr-tmp.{pid}.{n}.{file_name}");
    match dst.parent() {
        Some(parent) => parent.join(tmp_name),
        None => PathBuf::from(tmp_name),
    }
}

/// RAII guard that removes a temp file unless [`disarm`](TempCleanup::disarm)ed.
struct TempCleanup {
    path: Option<PathBuf>,
}

impl TempCleanup {
    fn disarm(mut self) {
        self.path = None;
    }
}

impl Drop for TempCleanup {
    fn drop(&mut self) {
        if let Some(p) = self.path.take() {
            let _ = fs::remove_file(p);
        }
    }
}

/// Run blocking I/O off the async reactor, mapping a join failure to a typed
/// error rather than panicking the caller.
async fn spawn_blocking<F, T>(f: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        Err(e) => Err(FsError::TaskJoin(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        let mut f = File::create(path).unwrap();
        f.write_all(bytes).unwrap();
    }

    /// Whether any `.cellarr-tmp.` staging debris is left in a directory. The
    /// "never leave a partial destination" property requires this to be false
    /// after both successful and failed copies.
    fn has_temp_debris(dir: &Path) -> bool {
        fs::read_dir(dir).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".cellarr-tmp.")
        })
    }

    #[test]
    fn durable_copy_places_full_contents_and_leaves_no_temp_debris() {
        let dir = tmpdir();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("sub/dst.bin");
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        write_file(&src, b"the full payload, durably copied");

        copy_durable_blocking(&src, &dst).unwrap();

        assert_eq!(fs::read(&dst).unwrap(), b"the full payload, durably copied");
        assert!(!has_temp_debris(dst.parent().unwrap()));
    }

    #[test]
    fn failed_copy_leaves_no_partial_destination_and_no_temp_debris() {
        // Force the final rename to fail by removing the destination directory
        // out from under the copier after the temp file is written. We simulate
        // this by pointing the destination at a path whose parent we delete
        // mid-flight is impractical synchronously; instead we trigger failure by
        // making the source unreadable partway is also hard. So we exercise the
        // RAII cleanup guard directly: a copy into a directory that is then made
        // un-renamable via a name collision with a directory.
        let dir = tmpdir();
        let src = dir.path().join("src.bin");
        write_file(&src, b"payload");

        // Destination path is actually an existing *directory*, so `rename` of a
        // file over a non-empty directory fails — exercising the failure branch.
        let dst = dir.path().join("dst_is_dir");
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("occupant"), b"x").unwrap();

        let err = copy_durable_blocking(&src, &dst);
        assert!(err.is_err(), "rename over a non-empty dir must fail");

        // The destination directory is unchanged (no partial file took its
        // place) and no temp debris is left beside it.
        assert!(dst.is_dir());
        assert!(!has_temp_debris(dir.path()));
    }

    #[tokio::test]
    async fn hardlink_within_filesystem_shares_inode() {
        let dir = tmpdir();
        let src = dir.path().join("a.bin");
        let dst = dir.path().join("b.bin");
        write_file(&src, b"shared");
        let outcome = hardlink_or_copy(&src, &dst).await.unwrap();
        assert_eq!(outcome, LinkOutcome::Hardlinked);
        assert_eq!(fs::read(&dst).unwrap(), b"shared");
        // Both names exist (seeding copy preserved).
        assert!(src.exists() && dst.exists());
    }

    #[tokio::test]
    async fn refuses_existing_destination() {
        let dir = tmpdir();
        let src = dir.path().join("a.bin");
        let dst = dir.path().join("b.bin");
        write_file(&src, b"x");
        write_file(&dst, b"y");
        let err = hardlink_or_copy(&src, &dst).await.unwrap_err();
        assert!(matches!(err, FsError::UnexpectedDestination { .. }));
    }

    #[tokio::test]
    async fn errors_on_missing_source() {
        let dir = tmpdir();
        let src = dir.path().join("nope.bin");
        let dst = dir.path().join("b.bin");
        let err = hardlink_or_copy(&src, &dst).await.unwrap_err();
        assert!(matches!(err, FsError::MissingPath { .. }));
    }

    #[tokio::test]
    async fn same_filesystem_is_true_within_a_tempdir() {
        let dir = tmpdir();
        let src = dir.path().join("a.bin");
        let dst = dir.path().join("nested/b.bin");
        write_file(&src, b"x");
        assert!(same_filesystem(&src, &dst).await.unwrap());
    }

    #[tokio::test]
    async fn remove_durable_is_idempotent() {
        let dir = tmpdir();
        let f = dir.path().join("f.bin");
        write_file(&f, b"x");
        remove_durable(&f).await.unwrap();
        assert!(!f.exists());
        // Removing again is still Ok.
        remove_durable(&f).await.unwrap();
    }
}
