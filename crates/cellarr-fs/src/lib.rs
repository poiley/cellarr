//! cellarr-fs — library file operations: scan, rename, and crash-safe import.
//!
//! This is the **only crate that can destroy user data**. It owns the
//! stage→verify→commit→cleanup discipline from `docs/03-pipeline.md` and treats
//! every change as safety-critical. The headline guarantees, enforced by tests:
//!
//! - A new file is fully durable (fsync) **before** any old file is removed.
//! - A cross-filesystem move never leaves a partial file at the destination.
//! - A crash at any phase leaves the library consistent and the operation
//!   resumable: re-running [`execute_import`] finishes the job.
//!
//! ## Surface
//! - [`scan`] — inventory an existing library root (read-only).
//! - [`plan_import`] — the **Stage** step: a pure plan, no mutation.
//! - [`execute_import`] — the **Verify→Commit→Cleanup** steps, crash-safe.
//! - [`recycle_or_delete`] — crash-safe removal of a content's media files,
//!   into the recycle bin (reversible) or by unlink, never outside the library.
//! - [`render_name`] — the deterministic rename engine.
//! - [`hardlink_or_copy`] — the durable placement primitive.
//! - [`check_same_filesystem`] — the loud cross-filesystem (silent-copy-fallback)
//!   health warning surfaced on `/api/v3/health`.
//!
//! See `docs/specs/cellarr-fs.md`.

#![forbid(unsafe_code)]

mod error;
mod fsops;
mod health;
mod import;
mod nfo;
mod recycle;
mod rename;
mod scan;

pub use error::{FsError, Result};
pub use fsops::{
    create_dir_all, file_size, hardlink_or_copy, remove_durable, same_filesystem, LinkOutcome,
};
pub use health::{check_same_filesystem, FilesystemWarning};
pub use import::{
    execute_import, execute_import_with, plan_import, CommitHooks, ImportResult, MoveResult,
    NoHooks, PlacedAs,
};
pub use nfo::{render_nfo, sidecar_path, write_sidecar, NfoKind, NfoMetadata};
pub use recycle::{recycle_or_delete, RecycleDisposition, RecycleResult};
pub use rename::{
    render_name, render_name_with, ColonReplacement, MultiEpisodeStyle, RenderOptions,
    TargetPlatform,
};
pub use scan::{scan, Inventory, InventoryEntry};
