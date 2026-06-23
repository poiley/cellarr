//! cellarr-migrate — import an existing Radarr/Sonarr install into cellarr.
//!
//! Migration is a day-one feature (docs/12-migration.md): a current *arr user
//! must bring their library and config into cellarr without re-scanning terabytes
//! or re-matching by hand. This crate reads the user's existing **SQLite**
//! database(s) **read-only** — the existing app keeps running — and maps the rows
//! into cellarr's schema:
//!
//! - library structure + identity (external TMDB/TVDB/IMDB ids preserved);
//! - file associations (files recognized **in place**, never moved);
//! - quality profiles + custom formats (mapped onto the same TRaSH-compatible
//!   decision model `cellarr-decide` uses, so decisions stay equivalent);
//! - indexers, download clients, and root folders (re-tested on import).
//!
//! # Shape of the API
//!
//! - [`detect_source`] — identify a database as Sonarr or Radarr by its schema.
//! - [`preview`] — read one or more sources and summarize what *would* import,
//!   **without writing** anything. Importing a Radarr DB and a Sonarr DB previews
//!   one unified library set (movies + TV side by side).
//! - [`import`] — perform the mapping into a fresh cellarr [`Database`]. Import is
//!   reversible: throw away the cellarr DB and re-import.
//! - [`recognize::plan_file_operations`] — prove the recognize-in-place
//!   guarantee: zero file operations for files already in place.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod config_read;
mod error;
mod model;
mod profiles;
mod radarr;
pub mod recognize;
mod sonarr;
mod source;
mod write;

use cellarr_core::MediaType;
use cellarr_db::Database;

pub use error::{MigrationError, Result};
pub use model::{ExternalIds, MappedContent, MappedFile, MappedInstall};
pub use source::SourceKind;

use source::Source;

/// A summary of what importing a set of sources would produce — no writes.
///
/// This is what a guided first-run import shows the user *before* committing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationPreview {
    /// The detected source kinds, one per input path, in input order.
    pub sources: Vec<SourceKind>,
    /// The media types the unified import would create libraries for.
    pub media_types: Vec<MediaType>,
    /// Number of libraries that would be created.
    pub library_count: usize,
    /// Number of content nodes (movies, or series+seasons+episodes).
    pub content_count: usize,
    /// Number of grabbable leaf nodes (movies / episodes) — the items a user
    /// thinks of as "things in my library".
    pub item_count: usize,
    /// Number of media files recognized in place.
    pub file_count: usize,
    /// Number of quality profiles mapped.
    pub profile_count: usize,
    /// Number of custom formats mapped.
    pub custom_format_count: usize,
    /// Number of indexers carried across.
    pub indexer_count: usize,
    /// Number of download clients carried across.
    pub download_client_count: usize,
    /// Number of root folders carried across.
    pub root_folder_count: usize,
    /// Number of file operations the import would schedule. Always **zero** for
    /// migration — files are recognized in place — and surfaced so the UI can
    /// state that plainly.
    pub scheduled_file_operations: usize,
}

/// The result of a completed import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// The preview the import was derived from (the same counts, now written).
    pub preview: MigrationPreview,
    /// Number of typed identity/metadata rows written (one per movie, series,
    /// and episode) — the preserved identities that spare cellarr from
    /// re-identifying the library.
    pub identities_written: usize,
}

/// Detect whether the database at `path` is a Sonarr or Radarr install.
///
/// Opens the file read-only; never writes.
///
/// # Errors
/// Returns [`MigrationError::Source`] if the file cannot be opened and
/// [`MigrationError::Unrecognized`] if it matches no known schema.
pub async fn detect_source(path: &str) -> Result<SourceKind> {
    Ok(Source::open(path).await?.kind())
}

/// Read every source path into one unified [`MappedInstall`], read-only.
async fn read_all(source_paths: &[&str]) -> Result<(MappedInstall, Vec<SourceKind>)> {
    let mut unified = MappedInstall::default();
    let mut kinds = Vec::with_capacity(source_paths.len());
    for path in source_paths {
        let source = Source::open(path).await?;
        kinds.push(source.kind());
        let mapped = match source.kind() {
            SourceKind::Radarr => radarr::read(source.pool()).await?,
            SourceKind::Sonarr => sonarr::read(source.pool()).await?,
        };
        unified.merge(mapped);
    }
    Ok((unified, kinds))
}

/// Summarize what importing `source_paths` would produce, **without writing**.
///
/// # Errors
/// Returns a [`MigrationError`] if any source cannot be read or mapped.
pub async fn preview(source_paths: &[&str]) -> Result<MigrationPreview> {
    let (unified, kinds) = read_all(source_paths).await?;
    Ok(summarize(&unified, kinds))
}

/// Build the preview summary from a mapped install.
fn summarize(unified: &MappedInstall, kinds: Vec<SourceKind>) -> MigrationPreview {
    let item_count = unified
        .contents
        .iter()
        .filter(|c| {
            matches!(
                c.node.kind,
                cellarr_core::ContentKind::Movie
                    | cellarr_core::ContentKind::Episode
                    | cellarr_core::ContentKind::Track
                    | cellarr_core::ContentKind::Book
            )
        })
        .count();

    // Migration always recognizes in place, so this is zero; computing it through
    // the same planner the import uses keeps the claim honest rather than assumed.
    let scheduled_file_operations =
        recognize::plan_file_operations(unified, &recognize::RecognizeInPlace).len();

    MigrationPreview {
        sources: kinds,
        media_types: unified.media_types(),
        library_count: unified.libraries.len(),
        content_count: unified.contents.len(),
        item_count,
        file_count: unified.files.len(),
        profile_count: unified.profiles.len(),
        custom_format_count: unified.custom_formats.len(),
        indexer_count: unified.indexers.len(),
        download_client_count: unified.download_clients.len(),
        root_folder_count: unified.root_folders.len(),
        scheduled_file_operations,
    }
}

/// Import `source_paths` into the destination cellarr `db`.
///
/// The sources are read read-only and the mapped data is written through the
/// destination's single writer-actor. No file is moved or deleted: every media
/// file is recognized at its existing path (the preview's
/// `scheduled_file_operations` is zero). The import is reversible — discard the
/// cellarr DB and re-run.
///
/// # Errors
/// Returns a [`MigrationError`] if any source cannot be read/mapped or the
/// destination write fails.
pub async fn import(source_paths: &[&str], db: &Database) -> Result<MigrationReport> {
    let (unified, kinds) = read_all(source_paths).await?;
    let preview = summarize(&unified, kinds);
    let identities_written = write::write_install(&unified, db).await?;
    Ok(MigrationReport {
        preview,
        identities_written,
    })
}
