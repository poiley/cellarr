//! Import: the stage→verify→commit→cleanup discipline.
//!
//! This module turns an [`ImportPlan`](cellarr_core::ImportPlan) — pure data
//! describing what *would* happen — into durable filesystem changes, following
//! the non-negotiable discipline from `docs/03-pipeline.md`:
//!
//! 1. **Stage** ([`plan_import`]) — compute every move; mutate nothing.
//! 2. **Verify** — re-confirm the sources exist and the destinations are safe
//!    *before* touching anything.
//! 3. **Commit** — place each new file durably (hardlink, or cross-fs copy +
//!    fsync + atomic rename) **before** any old file is removed.
//! 4. **Cleanup** — only now remove replaced files. (DB/history updates happen in
//!    the caller's transaction; this crate owns the bytes on disk.)
//!
//! ### Crash safety & resumability
//! Commit places files one at a time; each placement is individually atomic and
//! durable. If the process dies partway, the already-committed destinations are
//! real files and the not-yet-committed ones simply do not exist (a failed copy
//! leaves no partial — see [`crate::fsops`]). Re-running [`execute_import`] on
//! the same plan is therefore safe and finishes the job: a destination that is
//! already in place is recognized and skipped, and replaced-file removal is
//! idempotent. **No old file is ever removed until its replacement is durable.**

use std::path::{Path, PathBuf};

use cellarr_core::{ImportPlan, PlannedMove};

use crate::error::{FsError, Result};
use crate::fsops::{self, LinkOutcome};

/// The outcome of one committed move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveResult {
    /// The destination now holding the file.
    pub destination_path: PathBuf,
    /// How it was placed.
    pub outcome: PlacedAs,
    /// Whether a replaced file was removed during cleanup.
    pub removed_replaced: bool,
}

/// How a destination came to hold its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacedAs {
    /// A hardlink was created.
    Hardlinked,
    /// A durable copy was made.
    Copied,
    /// The file was already in place (a resumed/idempotent run).
    AlreadyPresent,
}

/// The result of executing an import plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportResult {
    /// One entry per planned move, in plan order.
    pub moves: Vec<MoveResult>,
}

/// Hooks that let callers (and crash-safety tests) observe — or abort — the
/// execution between phases. Production code uses [`NoHooks`]; tests inject a
/// hook that returns an error (or panics) at a chosen point to prove the library
/// stays consistent and the operation is resumable.
///
/// Each method's default is a no-op so real callers ignore them entirely.
pub trait CommitHooks: Send + Sync {
    /// Called after Verify, before any Commit mutation.
    ///
    /// # Errors
    /// Returning `Err` aborts before any mutation (the safest possible point).
    fn before_commit(&self) -> Result<()> {
        Ok(())
    }

    /// Called after each individual destination is durably committed, before the
    /// next one and before any cleanup. `index` is the move's position in the
    /// plan.
    ///
    /// # Errors
    /// Returning `Err` aborts after `index` destinations are durable but before
    /// any replaced file is removed — the critical ordering window.
    fn after_commit_one(&self, _index: usize) -> Result<()> {
        Ok(())
    }

    /// Called after all commits, before cleanup removes any replaced file.
    ///
    /// # Errors
    /// Returning `Err` aborts with every new file durable and every old file
    /// still present (a fully consistent, resumable state).
    fn before_cleanup(&self) -> Result<()> {
        Ok(())
    }
}

/// The production hook set: does nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHooks;
impl CommitHooks for NoHooks {}

/// **Stage.** Build the import plan: resolve each source to its destination and
/// decide hardlink-vs-copy, mutating nothing.
///
/// `moves` describes the intended placements (typically produced upstream from
/// the rename engine + decision engine). This function validates them against
/// the real filesystem (sources exist; hardlink feasibility is probed) and
/// returns a plan whose [`PlannedMove::hardlink`] flags reflect the actual
/// device layout — so Verify/Commit do not have to re-derive it.
///
/// # Errors
/// - [`FsError::MissingPath`] if a declared source does not exist.
/// - [`FsError::Io`] if device probing fails.
pub async fn plan_import(
    grab_id: cellarr_core::GrabId,
    moves: Vec<PlannedMove>,
) -> Result<ImportPlan> {
    let mut planned = Vec::with_capacity(moves.len());
    for mut m in moves {
        let src = PathBuf::from(&m.source_path);
        if !src.exists() {
            return Err(FsError::MissingPath { path: src });
        }
        let dst = PathBuf::from(&m.destination_path);
        // A hardlink is only possible within one filesystem. Probe and record
        // the truth so Commit doesn't guess.
        m.hardlink = fsops::same_filesystem(&src, &dst).await?;
        planned.push(m);
    }
    Ok(ImportPlan {
        grab_id,
        moves: planned,
    })
}

/// **Verify → Commit → Cleanup.** Execute an import plan durably and crash-safely.
///
/// Uses [`NoHooks`]; see [`execute_import_with`] to inject crash-safety hooks.
///
/// # Errors
/// Any [`FsError`]. On error, every destination committed so far remains durable
/// and no replaced file that still has a live replacement has been removed — the
/// plan can be re-run to completion.
pub async fn execute_import(plan: &ImportPlan) -> Result<ImportResult> {
    execute_import_with(plan, &NoHooks).await
}

/// Like [`execute_import`] but with caller-supplied [`CommitHooks`].
///
/// # Errors
/// See [`execute_import`].
pub async fn execute_import_with<H: CommitHooks>(
    plan: &ImportPlan,
    hooks: &H,
) -> Result<ImportResult> {
    // --- Verify (no mutation) ---------------------------------------------
    for m in &plan.moves {
        verify_move(m).await?;
    }
    hooks.before_commit()?;

    // --- Commit (new files durable; old files untouched) ------------------
    // We deliberately commit ALL new files before removing ANY old file, so the
    // ordering invariant holds across the whole plan, not just per-move.
    let mut placed = Vec::with_capacity(plan.moves.len());
    for (i, m) in plan.moves.iter().enumerate() {
        let outcome = commit_move(m).await?;
        placed.push(outcome);
        hooks.after_commit_one(i)?;
    }

    hooks.before_cleanup()?;

    // --- Cleanup (remove replaced files; idempotent) ----------------------
    let mut results = Vec::with_capacity(plan.moves.len());
    for (m, outcome) in plan.moves.iter().zip(placed) {
        let removed = cleanup_move(m).await?;
        results.push(MoveResult {
            destination_path: PathBuf::from(&m.destination_path),
            outcome,
            removed_replaced: removed,
        });
    }

    Ok(ImportResult { moves: results })
}

/// Verify a single move can proceed: the source exists; the destination is
/// either absent, or already correctly in place (resume), or an expected
/// replacement. An unexpected occupied destination is refused.
async fn verify_move(m: &PlannedMove) -> Result<()> {
    let src = PathBuf::from(&m.source_path);
    let dst = PathBuf::from(&m.destination_path);

    if !path_exists(&src).await? {
        // On a *resumed* run the source may legitimately be gone if this move is
        // already fully committed (a copy could have removed nothing, but a move
        // semantics caller might). We only tolerate a missing source when the
        // destination already holds the file.
        if destination_already_satisfied(m).await? {
            return Ok(());
        }
        return Err(FsError::MissingPath { path: src });
    }

    if path_exists(&dst).await? && !destination_already_satisfied(m).await? && m.replaces.is_none()
    {
        return Err(FsError::UnexpectedDestination { path: dst });
    }
    Ok(())
}

/// Place one new file durably. Recognizes an already-committed destination
/// (resume) and skips it. Never removes the source here — the seeding copy is
/// preserved; replaced files are handled in Cleanup, after this returns durable.
async fn commit_move(m: &PlannedMove) -> Result<PlacedAs> {
    let src = PathBuf::from(&m.source_path);
    let dst = PathBuf::from(&m.destination_path);

    if destination_already_satisfied(m).await? {
        return Ok(PlacedAs::AlreadyPresent);
    }

    // Ensure the destination directory tree exists before placing the file.
    if let Some(parent) = dst.parent() {
        fsops::create_dir_all(parent).await?;
    }

    // If a replacement target occupies the destination path, we move the new
    // file in under a temp name and atomically swap, so the old file is only
    // unlinked by the rename once the new bytes are durable — never a window
    // where neither exists.
    if path_exists(&dst).await? {
        return commit_replacing(&src, &dst).await;
    }

    match fsops::hardlink_or_copy(&src, &dst).await? {
        LinkOutcome::Hardlinked => Ok(PlacedAs::Hardlinked),
        LinkOutcome::Copied => Ok(PlacedAs::Copied),
    }
}

/// Commit a move whose destination path is currently occupied by the file being
/// replaced. We stage the new file beside the destination, fsync it, then a
/// single atomic rename replaces the old file. At no instant is the destination
/// absent or partial.
async fn commit_replacing(src: &Path, dst: &Path) -> Result<PlacedAs> {
    let staged = staging_path(dst);
    // Place the new file durably at the staging path first.
    let outcome = fsops::hardlink_or_copy(src, &staged).await?;
    // Atomic replace: rename over the existing destination.
    let staged_clone = staged.clone();
    let dst_clone = dst.to_path_buf();
    let rename = tokio::task::spawn_blocking(move || {
        std::fs::rename(&staged_clone, &dst_clone).map_err(|e| FsError::io(&dst_clone, e))
    })
    .await
    .map_err(|e| FsError::TaskJoin(e.to_string()))?;
    if let Err(e) = rename {
        // Clean up the staged file so we never leave debris.
        let _ = fsops::remove_durable(&staged).await;
        return Err(e);
    }
    Ok(match outcome {
        LinkOutcome::Hardlinked => PlacedAs::Hardlinked,
        LinkOutcome::Copied => PlacedAs::Copied,
    })
}

/// Cleanup one move: remove a replaced file that sits at a path *distinct* from
/// the destination. Returns whether a file was removed. Idempotent: a missing
/// replaced file is success (a resumed cleanup must not fail on already-removed
/// debris).
///
/// Cleanup runs only after every destination in the plan is durable, so removing
/// the old file here can never violate the new-before-old ordering. An in-place
/// replacement (`replaced_path == destination_path`, or unset) was already
/// atomically consumed by `commit_replacing`, so there is nothing to delete.
async fn cleanup_move(m: &PlannedMove) -> Result<bool> {
    let Some(replaced) = &m.replaced_path else {
        return Ok(false);
    };

    // An upgrade that lands at the same path overwrote the old file in place via
    // the atomic rename in `commit_replacing`; there is no separate file left.
    if replaced == &m.destination_path {
        return Ok(false);
    }

    let replaced = PathBuf::from(replaced);

    // Safety backstop: only remove the replaced file once the new file is
    // genuinely durable at the destination. Cleanup is reached only after Commit,
    // but on a *resumed* run we re-confirm rather than trust the prior process.
    if !path_exists(&PathBuf::from(&m.destination_path)).await? {
        return Err(FsError::VerificationFailed {
            path: PathBuf::from(&m.destination_path),
            detail: "refusing to remove the replaced file: destination is not in place".into(),
        });
    }

    // Idempotent: a re-run after the file was already removed must succeed.
    if !path_exists(&replaced).await? {
        return Ok(false);
    }

    fsops::remove_durable(&replaced).await?;
    Ok(true)
}

/// Whether the destination already holds the intended file (same size as the
/// source, or the source no longer exists but the destination does). This is the
/// resume/idempotency check: a re-run must recognize already-committed work and
/// not redo or, worse, clobber it.
async fn destination_already_satisfied(m: &PlannedMove) -> Result<bool> {
    let src = PathBuf::from(&m.source_path);
    let dst = PathBuf::from(&m.destination_path);
    if !path_exists(&dst).await? {
        return Ok(false);
    }
    // If this move replaces an existing library file, an occupied destination is
    // expected and must NOT be treated as "already satisfied" until the new
    // bytes are in place. Compare sizes to distinguish.
    let dst_size = size_of(&dst).await?;
    if path_exists(&src).await? {
        let src_size = size_of(&src).await?;
        Ok(src_size == dst_size)
    } else {
        // Source gone, destination present: a prior run committed it.
        Ok(true)
    }
}

/// A staging path beside the destination for the atomic-replace path.
fn staging_path(dst: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = dst
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let staged = format!(".cellarr-staging.{}.{}.{}", std::process::id(), n, name);
    dst.parent()
        .map(|p| p.join(&staged))
        .unwrap_or_else(|| PathBuf::from(staged))
}

async fn path_exists(p: &Path) -> Result<bool> {
    let p = p.to_path_buf();
    tokio::task::spawn_blocking(move || p.exists())
        .await
        .map_err(|e| FsError::TaskJoin(e.to_string()))
}

async fn size_of(p: &Path) -> Result<u64> {
    let p = p.to_path_buf();
    tokio::task::spawn_blocking(move || fsops::file_size(&p))
        .await
        .map_err(|e| FsError::TaskJoin(e.to_string()))?
}
