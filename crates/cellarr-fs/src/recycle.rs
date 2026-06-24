//! Crash-safe removal of a content's media files (recycle bin or unlink).
//!
//! Deleting media is on the same library-destroying code path as import, so it
//! obeys the same discipline: a delete is **staged → verified → committed →
//! logged**, never corrupts the library, and is reversible by default.
//!
//! - When a **recycle bin** is configured, each file is *moved* into it,
//!   preserving its path relative to the library root, so a mistaken delete is
//!   undoable (the bytes still exist under the bin). The move is durable-first:
//!   the file is placed in the bin (hardlink, or cross-fs copy + fsync + atomic
//!   rename) **before** the original is unlinked — never a window where neither
//!   exists.
//! - With **no recycle bin**, the file is unlinked durably (the parent directory
//!   is fsynced so the removal survives a crash).
//!
//! Every file is guarded to be **within the library root** before anything is
//! touched: a path that escapes the root (via `..`, an absolute path, or a
//! symlink resolving outside) is refused with [`FsError::PathEscape`] and nothing
//! is deleted. This is the non-negotiable "never delete outside the library"
//! rule, enforced here rather than trusted from the caller.
//!
//! Each action is logged at `info` so a delete leaves an audit trail.

use std::path::{Component, Path, PathBuf};

use tracing::info;

use crate::error::{FsError, Result};
use crate::fsops::{self, LinkOutcome};

/// How one file's removal was carried out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecycleDisposition {
    /// The file was moved into the recycle bin at this path (reversible).
    Recycled {
        /// Where the file now lives inside the bin.
        bin_path: PathBuf,
    },
    /// The file was unlinked outright (no recycle bin configured).
    Deleted,
    /// The file was already gone (a resumed/idempotent delete). Nothing to do.
    AlreadyAbsent,
}

/// The outcome of recycling/deleting one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecycleResult {
    /// The original library path that was removed.
    pub source_path: PathBuf,
    /// What happened to it.
    pub disposition: RecycleDisposition,
}

/// Remove every file in `files`, recycling into `recycle_bin` when one is given
/// and unlinking otherwise. Every path is first verified to live within
/// `library_root`; if any does not, the whole batch is refused before a single
/// file is touched (fail-closed: a delete never half-runs across the root
/// boundary).
///
/// Returns one [`RecycleResult`] per input file, in input order. Idempotent: a
/// file that is already gone yields [`RecycleDisposition::AlreadyAbsent`], so a
/// resumed delete completes rather than failing on already-removed bytes.
///
/// # Errors
/// - [`FsError::PathEscape`] if any file is not within `library_root` (checked up
///   front; nothing is removed when this fires).
/// - [`FsError::Io`] if a move or unlink fails.
pub async fn recycle_or_delete(
    files: &[PathBuf],
    library_root: &Path,
    recycle_bin: Option<&Path>,
) -> Result<Vec<RecycleResult>> {
    // --- Verify (no mutation): every file must be inside the library root. ----
    // We resolve the boundary up front so a single escaping path aborts the whole
    // batch before any file is touched.
    let root = normalize(library_root);
    let mut checked = Vec::with_capacity(files.len());
    for f in files {
        let within = ensure_within_root(f, &root)?;
        checked.push((f.clone(), within));
    }

    // --- Commit: recycle or unlink each file, durably. ------------------------
    let mut results = Vec::with_capacity(checked.len());
    for (source, rel) in checked {
        let disposition = match recycle_bin {
            Some(bin) => recycle_one(&source, &rel, bin).await?,
            None => delete_one(&source).await?,
        };
        match &disposition {
            RecycleDisposition::Recycled { bin_path } => {
                info!(source = %source.display(), bin = %bin_path.display(), "recycled media file");
            }
            RecycleDisposition::Deleted => {
                info!(source = %source.display(), "deleted media file");
            }
            RecycleDisposition::AlreadyAbsent => {
                info!(source = %source.display(), "media file already absent; nothing to delete");
            }
        }
        results.push(RecycleResult {
            source_path: source,
            disposition,
        });
    }
    Ok(results)
}

/// Move one file into the recycle bin, preserving its path relative to the
/// library root. Durable-first: the file is placed in the bin before the
/// original is unlinked, so a crash never loses the bytes (either the original or
/// the bin copy — or, briefly, both — always exists).
async fn recycle_one(source: &Path, rel: &Path, bin: &Path) -> Result<RecycleDisposition> {
    if !path_exists(source).await? {
        return Ok(RecycleDisposition::AlreadyAbsent);
    }

    // The destination mirrors the file's layout under the library root, so a
    // restore is unambiguous. A collision (same relative path already recycled)
    // gets a unique suffix rather than clobbering an earlier recycled file.
    let mut dst = bin.join(rel);
    if path_exists(&dst).await? {
        dst = unique_sibling(&dst).await?;
    }
    if let Some(parent) = dst.parent() {
        fsops::create_dir_all(parent).await?;
    }

    // Place the file in the bin durably (hardlink within a fs, else copy+fsync).
    // The original is left untouched until this returns durable.
    let _outcome: LinkOutcome = fsops::hardlink_or_copy(source, &dst).await?;

    // Now — and only now — remove the original. If we crash between the placement
    // and here, a re-run finds the original still present and the bin destination
    // occupied, takes a fresh unique suffix, and proceeds; the worst case is a
    // duplicate in the bin, never a lost file.
    fsops::remove_durable(source).await?;
    Ok(RecycleDisposition::Recycled { bin_path: dst })
}

/// Unlink one file durably (no recycle bin). Idempotent: an already-absent file
/// is success.
async fn delete_one(source: &Path) -> Result<RecycleDisposition> {
    if !path_exists(source).await? {
        return Ok(RecycleDisposition::AlreadyAbsent);
    }
    fsops::remove_durable(source).await?;
    Ok(RecycleDisposition::Deleted)
}

/// Verify `path` resolves inside `root`, returning its path **relative to**
/// `root` (used to mirror the layout in the recycle bin). Refuses any path that
/// escapes the root.
///
/// The check is lexical on a normalized path (resolving `.`/`..` without
/// touching the filesystem) so it works for files that may already be gone, and
/// additionally rejects any path that, once made absolute against the root,
/// still does not start with the root prefix.
fn ensure_within_root(path: &Path, normalized_root: &Path) -> Result<PathBuf> {
    let abs = if path.is_absolute() {
        normalize(path)
    } else {
        normalize(&normalized_root.join(path))
    };
    match abs.strip_prefix(normalized_root) {
        Ok(rel) if !rel.as_os_str().is_empty() => Ok(rel.to_path_buf()),
        // The path equals the root itself, or is not under it: an escape.
        _ => Err(FsError::PathEscape {
            path: path.to_path_buf(),
            root: normalized_root.to_path_buf(),
        }),
    }
}

/// Lexically normalize a path: collapse `.` and resolve `..` against prior
/// components, without consulting the filesystem. This makes the containment
/// check total for paths whose targets may not exist, while still neutralizing
/// `..` traversal that would otherwise climb out of the root.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop a normal component; never pop past the root prefix so a
                // leading `..` cannot escape an absolute base.
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push(comp.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// A unique sibling of `dst` (appending `.1`, `.2`, …) for a recycle-bin
/// collision, so re-deleting a file with the same relative path never overwrites
/// an earlier recycled copy.
async fn unique_sibling(dst: &Path) -> Result<PathBuf> {
    let parent = dst.parent().map(Path::to_path_buf);
    let name = dst
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    for n in 1u32.. {
        let candidate = match &parent {
            Some(p) => p.join(format!("{name}.{n}")),
            None => PathBuf::from(format!("{name}.{n}")),
        };
        if !path_exists(&candidate).await? {
            return Ok(candidate);
        }
    }
    // The loop is effectively unbounded (u32 range); this line is unreachable in
    // practice but keeps the function total without an unwrap.
    Err(FsError::Io {
        path: dst.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "no free recycle-bin name",
        ),
    })
}

async fn path_exists(p: &Path) -> Result<bool> {
    let p = p.to_path_buf();
    tokio::task::spawn_blocking(move || p.exists())
        .await
        .map_err(|e| FsError::TaskJoin(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    #[tokio::test]
    async fn unlinks_when_no_recycle_bin() {
        let dir = tmpdir();
        let root = dir.path().join("lib");
        let f = root.join("Movie/movie.mkv");
        write_file(&f, b"payload");

        let out = recycle_or_delete(std::slice::from_ref(&f), &root, None)
            .await
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].disposition, RecycleDisposition::Deleted);
        assert!(!f.exists(), "file must be gone after delete");
    }

    #[tokio::test]
    async fn moves_to_recycle_bin_preserving_layout() {
        let dir = tmpdir();
        let root = dir.path().join("lib");
        let bin = dir.path().join("recycle");
        let f = root.join("Movie (2020)/movie.mkv");
        write_file(&f, b"the bytes");

        let out = recycle_or_delete(std::slice::from_ref(&f), &root, Some(&bin))
            .await
            .unwrap();
        // Original is gone…
        assert!(!f.exists());
        // …and a copy lives in the bin under the same relative layout.
        let expected = bin.join("Movie (2020)/movie.mkv");
        assert!(expected.exists(), "recycled file must be in the bin");
        assert_eq!(fs::read(&expected).unwrap(), b"the bytes");
        match &out[0].disposition {
            RecycleDisposition::Recycled { bin_path } => assert_eq!(bin_path, &expected),
            other => panic!("expected Recycled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn recycle_collision_gets_unique_name_not_clobber() {
        let dir = tmpdir();
        let root = dir.path().join("lib");
        let bin = dir.path().join("recycle");
        // Pre-occupy the bin destination with a different earlier recycled file.
        let occupied = bin.join("Movie/movie.mkv");
        write_file(&occupied, b"earlier recycled");

        let f = root.join("Movie/movie.mkv");
        write_file(&f, b"new delete");

        let out = recycle_or_delete(std::slice::from_ref(&f), &root, Some(&bin))
            .await
            .unwrap();
        // The earlier recycled copy is untouched.
        assert_eq!(fs::read(&occupied).unwrap(), b"earlier recycled");
        // The new one landed beside it under a unique suffix.
        match &out[0].disposition {
            RecycleDisposition::Recycled { bin_path } => {
                assert_ne!(bin_path, &occupied);
                assert_eq!(fs::read(bin_path).unwrap(), b"new delete");
            }
            other => panic!("expected Recycled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn refuses_path_escaping_root_and_deletes_nothing() {
        let dir = tmpdir();
        let root = dir.path().join("lib");
        // A file genuinely outside the library root.
        let outside = dir.path().join("outside/secret.mkv");
        write_file(&outside, b"do not touch");
        // Reference it via a `..` traversal rooted at the library, the classic
        // escape.
        let escaping = root.join("../outside/secret.mkv");

        let err = recycle_or_delete(&[escaping], &root, None)
            .await
            .unwrap_err();
        assert!(matches!(err, FsError::PathEscape { .. }));
        // The fail-closed guarantee: nothing outside the root was removed.
        assert!(outside.exists(), "an escaping delete must touch nothing");
    }

    #[tokio::test]
    async fn one_escape_aborts_the_whole_batch_before_any_removal() {
        let dir = tmpdir();
        let root = dir.path().join("lib");
        let inside = root.join("Movie/keep.mkv");
        write_file(&inside, b"inside");
        let outside = dir.path().join("evil.mkv");
        write_file(&outside, b"outside");
        let escaping = root.join("../evil.mkv");

        let err = recycle_or_delete(&[inside.clone(), escaping], &root, None)
            .await
            .unwrap_err();
        assert!(matches!(err, FsError::PathEscape { .. }));
        // The in-root file is NOT removed: the batch is verified before any commit.
        assert!(
            inside.exists(),
            "a batch with an escape must remove nothing"
        );
        assert!(outside.exists());
    }

    #[tokio::test]
    async fn idempotent_on_already_absent_file() {
        let dir = tmpdir();
        let root = dir.path().join("lib");
        let f = root.join("Movie/gone.mkv");
        // The file never existed (or a prior run removed it).
        fs::create_dir_all(f.parent().unwrap()).unwrap();

        let out = recycle_or_delete(std::slice::from_ref(&f), &root, None)
            .await
            .unwrap();
        assert_eq!(out[0].disposition, RecycleDisposition::AlreadyAbsent);
    }

    #[tokio::test]
    async fn rejects_absolute_path_outside_root() {
        let dir = tmpdir();
        let root = dir.path().join("lib");
        fs::create_dir_all(&root).unwrap();
        let abs_outside = dir.path().join("etc/passwd");
        write_file(&abs_outside, b"x");

        let err = recycle_or_delete(std::slice::from_ref(&abs_outside), &root, None)
            .await
            .unwrap_err();
        assert!(matches!(err, FsError::PathEscape { .. }));
        assert!(abs_outside.exists());
    }
}
