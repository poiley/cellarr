//! Crash-safety and durability tests for the import committer.
//!
//! These are the load-bearing tests for the crate's reason to exist: that an
//! import can crash at any phase without corrupting the library, that a new file
//! is always durable before an old one is removed, and that a re-run finishes
//! the job. Failure is injected via [`CommitHooks`] returning an error (a stand-
//! in for a process death — same observable on-disk state, since the hook fires
//! at exactly the phase boundary we care about).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use cellarr_core::{ContentId, GrabId, MediaFileId, PlannedMove};
use cellarr_fs::{execute_import, execute_import_with, plan_import, CommitHooks, PlacedAs, Result};
use tempfile::TempDir;

// --- helpers --------------------------------------------------------------

fn write(path: &Path, contents: &[u8]) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

fn planned(src: &Path, dst: &Path, replaces: Option<MediaFileId>) -> PlannedMove {
    PlannedMove {
        source_path: src.to_string_lossy().into_owned(),
        destination_path: dst.to_string_lossy().into_owned(),
        content_ids: vec![ContentId::new()],
        replaces,
        // plan_import recomputes this; default for direct construction.
        hardlink: false,
    }
}

/// A hook that fails on a chosen phase. `fail_after_commit_index = Some(n)`
/// aborts right after the n-th destination is durable (the critical ordering
/// window). Other knobs abort at the other phase boundaries.
struct FailAt {
    before_commit: bool,
    fail_after_commit_index: Option<usize>,
    before_cleanup: bool,
    commits_observed: AtomicUsize,
}

impl FailAt {
    fn before_commit() -> Self {
        Self {
            before_commit: true,
            fail_after_commit_index: None,
            before_cleanup: false,
            commits_observed: AtomicUsize::new(0),
        }
    }
    fn after_commit(index: usize) -> Self {
        Self {
            before_commit: false,
            fail_after_commit_index: Some(index),
            before_cleanup: false,
            commits_observed: AtomicUsize::new(0),
        }
    }
    fn before_cleanup() -> Self {
        Self {
            before_commit: false,
            fail_after_commit_index: None,
            before_cleanup: true,
            commits_observed: AtomicUsize::new(0),
        }
    }
}

fn injected() -> cellarr_fs::FsError {
    cellarr_fs::FsError::TaskJoin("injected crash".into())
}

impl CommitHooks for FailAt {
    fn before_commit(&self) -> Result<()> {
        if self.before_commit {
            return Err(injected());
        }
        Ok(())
    }
    fn after_commit_one(&self, index: usize) -> Result<()> {
        self.commits_observed.fetch_add(1, Ordering::SeqCst);
        if self.fail_after_commit_index == Some(index) {
            return Err(injected());
        }
        Ok(())
    }
    fn before_cleanup(&self) -> Result<()> {
        if self.before_cleanup {
            return Err(injected());
        }
        Ok(())
    }
}

// --- tests ----------------------------------------------------------------

#[tokio::test]
async fn happy_path_hardlinks_within_one_filesystem_and_preserves_source() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("download/show.s01e01.mkv");
    let dst = tmp.path().join("library/Show/Season 01/Show - S01E01.mkv");
    write(&src, b"episode bytes");

    let plan = plan_import(GrabId::new(), vec![planned(&src, &dst, None)])
        .await
        .unwrap();
    // Same temp dir => same filesystem => hardlink feasible.
    assert!(plan.moves[0].hardlink);

    let result = execute_import(&plan).await.unwrap();
    assert_eq!(result.moves[0].outcome, PlacedAs::Hardlinked);

    // New file in place AND the seeding source still present (hardlink).
    assert_eq!(read(&dst), b"episode bytes");
    assert!(src.exists(), "seeding copy must be preserved");
}

#[tokio::test]
async fn crash_before_commit_leaves_library_untouched_and_is_resumable() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("download/a.mkv");
    let dst = tmp.path().join("library/a.mkv");
    write(&src, b"data");
    let plan = plan_import(GrabId::new(), vec![planned(&src, &dst, None)])
        .await
        .unwrap();

    let err = execute_import_with(&plan, &FailAt::before_commit()).await;
    assert!(err.is_err());
    // No mutation happened at all.
    assert!(
        !dst.exists(),
        "nothing should be committed before the crash"
    );
    assert!(src.exists());

    // Resume: a normal run completes.
    let result = execute_import(&plan).await.unwrap();
    assert_eq!(read(&dst), b"data");
    assert_ne!(result.moves[0].outcome, PlacedAs::AlreadyPresent);
}

#[tokio::test]
async fn crash_midway_through_commit_leaves_committed_files_and_resumes() {
    let tmp = TempDir::new().unwrap();
    let s0 = tmp.path().join("dl/0.mkv");
    let s1 = tmp.path().join("dl/1.mkv");
    let s2 = tmp.path().join("dl/2.mkv");
    let d0 = tmp.path().join("lib/0.mkv");
    let d1 = tmp.path().join("lib/1.mkv");
    let d2 = tmp.path().join("lib/2.mkv");
    write(&s0, b"zero");
    write(&s1, b"one");
    write(&s2, b"two");

    let plan = plan_import(
        GrabId::new(),
        vec![
            planned(&s0, &d0, None),
            planned(&s1, &d1, None),
            planned(&s2, &d2, None),
        ],
    )
    .await
    .unwrap();

    // Crash right after the second destination (index 1) is durable.
    let err = execute_import_with(&plan, &FailAt::after_commit(1)).await;
    assert!(err.is_err());

    // The first two are durable; the third was never created.
    assert_eq!(read(&d0), b"zero");
    assert_eq!(read(&d1), b"one");
    assert!(!d2.exists(), "uncommitted destination must not exist");

    // Resume: the run finishes, recognizing the already-present files.
    let result = execute_import(&plan).await.unwrap();
    assert_eq!(read(&d2), b"two");
    assert_eq!(result.moves[0].outcome, PlacedAs::AlreadyPresent);
    assert_eq!(result.moves[1].outcome, PlacedAs::AlreadyPresent);
}

#[tokio::test]
async fn old_file_is_never_removed_before_new_file_is_durable() {
    // Replace an existing inferior library file. We crash in the window AFTER
    // the new file is durable but BEFORE cleanup. The destination must hold the
    // NEW content (durable) and nothing must be missing.
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("dl/new.mkv");
    let dst = tmp.path().join("lib/movie.mkv");
    write(&src, b"the upgraded 1080p file");
    write(&dst, b"the old 720p file"); // existing inferior file at the dest

    let plan = plan_import(
        GrabId::new(),
        vec![planned(&src, &dst, Some(MediaFileId::new()))],
    )
    .await
    .unwrap();

    // Abort just before cleanup: new bytes must already be durable at dst.
    let err = execute_import_with(&plan, &FailAt::before_cleanup()).await;
    assert!(err.is_err());
    assert!(dst.exists(), "destination must never be absent");
    assert_eq!(
        read(&dst),
        b"the upgraded 1080p file",
        "the new file must be durable at the destination before any removal"
    );

    // Resume completes cleanly and the destination still holds the new file.
    let result = execute_import(&plan).await.unwrap();
    assert_eq!(read(&dst), b"the upgraded 1080p file");
    assert_eq!(result.moves.len(), 1);
}

#[tokio::test]
async fn refuses_to_clobber_an_unplanned_destination() {
    // A destination already occupied by a file the plan did NOT mark as replaced
    // must be refused at Verify — we never silently overwrite.
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("dl/x.mkv");
    let dst = tmp.path().join("lib/x.mkv");
    write(&src, b"new and different content here");
    write(&dst, b"precious unrelated existing file"); // different size

    let plan = plan_import(GrabId::new(), vec![planned(&src, &dst, None)])
        .await
        .unwrap();
    let err = execute_import(&plan).await;
    assert!(err.is_err(), "must refuse to clobber an unplanned dest");
    // The existing file is untouched.
    assert_eq!(read(&dst), b"precious unrelated existing file");
}

#[tokio::test]
async fn plan_import_errors_on_missing_source() {
    let tmp = TempDir::new().unwrap();
    let missing = tmp.path().join("dl/gone.mkv");
    let dst = tmp.path().join("lib/gone.mkv");
    let err = plan_import(GrabId::new(), vec![planned(&missing, &dst, None)]).await;
    assert!(matches!(err, Err(cellarr_fs::FsError::MissingPath { .. })));
}

#[tokio::test]
async fn execute_is_idempotent_when_already_complete() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("dl/a.mkv");
    let dst = tmp.path().join("lib/a.mkv");
    write(&src, b"content");
    let plan = plan_import(GrabId::new(), vec![planned(&src, &dst, None)])
        .await
        .unwrap();

    execute_import(&plan).await.unwrap();
    // Second run is a no-op that still succeeds.
    let result = execute_import(&plan).await.unwrap();
    assert_eq!(result.moves[0].outcome, PlacedAs::AlreadyPresent);
    assert_eq!(read(&dst), b"content");
}
