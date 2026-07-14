//! Extra-file (sidecar) import: subtitles and friends, alongside the media.
//!
//! When importing `Show.S01E01.mkv`, a release often ships sibling files that
//! belong to it — `Show.S01E01.en.srt`, `Show.S01E01.idx`, `Show.S01E01.nfo`.
//! With the media-management *import extra files* setting enabled, this module
//! finds those siblings (same basename, a configured extension) and places them
//! next to the **renamed** media, carrying the media's new basename and the
//! sibling's language/format suffix: `… - S01E01.en.srt`.
//!
//! ### Best-effort, never library-critical
//! This runs **after** the media is durably committed by
//! [`execute_import`](crate::execute_import). It is a pure addition: each extra is
//! placed via the same durable [`hardlink_or_copy`](crate::hardlink_or_copy)
//! primitive (no partial destination on failure), and a failure on any single
//! extra is recorded and skipped — it never rolls back, corrupts, or fails the
//! media import. The worst case is a missing subtitle, which a re-run re-imports.

use std::path::{Path, PathBuf};

use cellarr_core::ExtraFileImport;

use crate::error::FsError;
use crate::fsops::{self, LinkOutcome};

/// The outcome of one extra file's import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtraOutcome {
    /// The extra was placed at `destination`.
    Imported {
        /// Where the extra was placed.
        destination: PathBuf,
        /// Whether it was hardlinked (vs. copied).
        hardlinked: bool,
    },
    /// The extra was **not** placed because its destination already exists, and we
    /// never clobber (subtitles are best-effort; the media path's no-clobber safety
    /// applies here too). This is the benign, expected case on a re-import, an
    /// adopt-in-place (the source directory *is* the library directory), or a
    /// quality upgrade — not a failure.
    Skipped {
        /// The destination that was left untouched.
        destination: PathBuf,
        /// Whether the existing destination is already the very same file as the
        /// source (identical bytes / a shared inode) versus a different file that
        /// happens to occupy the target name. Both are left in place; this only
        /// distinguishes the log detail.
        identical: bool,
    },
    /// Importing this extra failed for a real reason (a filesystem error creating
    /// the directory or copying); the media import is unaffected. Logged at warn.
    Failed {
        /// The source extra that could not be placed.
        source: PathBuf,
        /// Why (already formatted for a log line).
        reason: String,
    },
}

/// Find and import the sibling extra files belonging to `media_source`, placing
/// each next to `media_destination` with the destination's basename.
///
/// `media_source` is the original media file (its directory is searched for
/// siblings); `media_destination` is where that media now lives (already
/// committed). Returns one [`ExtraOutcome`] per discovered extra. When the policy
/// is disabled, or there are no matching siblings, the result is empty.
///
/// Never returns an error: the media import already succeeded, so any per-extra
/// failure is folded into [`ExtraOutcome::Failed`] for the caller to log.
pub async fn import_extras(
    media_source: impl AsRef<Path>,
    media_destination: impl AsRef<Path>,
    policy: &ExtraFileImport,
) -> Vec<ExtraOutcome> {
    if !policy.enabled {
        return Vec::new();
    }
    let media_source = media_source.as_ref().to_path_buf();
    let media_destination = media_destination.as_ref().to_path_buf();
    let policy = policy.clone();

    let plan = match siblings_for(&media_source, &media_destination, &policy) {
        Ok(plan) => plan,
        Err(_) => return Vec::new(),
    };

    let mut outcomes = Vec::with_capacity(plan.len());
    for (source, destination) in plan {
        // Idempotent + safe: if the extra is already at its destination, never
        // clobber it — report Skipped, not Failed. This is the common, benign case
        // on a re-import, an adopt-in-place (the plan's source *is* the library
        // file, so its sibling subtitle is already correctly named beside it), and
        // a quality upgrade. Distinguish "already the same file" from "a different
        // file occupies the name" only for the log; both are left in place, matching
        // the media path's no-clobber guarantee.
        if destination.exists() {
            let identical = files_are_identical(&source, &destination);
            outcomes.push(ExtraOutcome::Skipped {
                destination,
                identical,
            });
            continue;
        }
        // The media destination directory already exists (the media landed there),
        // but a defensive create keeps this independent of placement order.
        if let Some(parent) = destination.parent() {
            if let Err(e) = fsops::create_dir_all(parent).await {
                outcomes.push(ExtraOutcome::Failed {
                    source,
                    reason: format!("create extra dir: {e}"),
                });
                continue;
            }
        }
        match fsops::hardlink_or_copy(&source, &destination).await {
            Ok(LinkOutcome::Hardlinked) => outcomes.push(ExtraOutcome::Imported {
                destination,
                hardlinked: true,
            }),
            Ok(LinkOutcome::Copied) => outcomes.push(ExtraOutcome::Imported {
                destination,
                hardlinked: false,
            }),
            Err(e) => outcomes.push(ExtraOutcome::Failed {
                source,
                reason: e.to_string(),
            }),
        }
    }
    outcomes
}

/// Whether `a` and `b` are the same file or hold identical content — used only to
/// label a [`ExtraOutcome::Skipped`] (both are left untouched regardless).
///
/// Cheap and best-effort: the same path or a shared inode is decisive without
/// reading bytes; otherwise, equal-length files are compared by content (extras
/// are small — subtitles, `.nfo`). Any stat/read error is treated as "not
/// identical" (the conservative label), never propagated.
fn files_are_identical(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) {
            // A shared inode on one device is literally one file (a prior hardlink
            // import); differing sizes cannot be identical.
            if ma.dev() == mb.dev() && ma.ino() == mb.ino() {
                return true;
            }
            if ma.len() != mb.len() {
                return false;
            }
        }
    }
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Compute the (source → destination) pairs for the extras belonging to
/// `media_source`, renamed to sit beside `media_destination`.
///
/// A sibling qualifies when, in the media's source directory, its file name
/// *starts with the media source stem* and *ends with a configured extra
/// extension*. The "suffix" between the stem and the matched extension (e.g.
/// `.en` in `Show.S01E01.en.srt`, or empty for `Show.S01E01.srt`) is preserved and
/// appended to the destination stem, so language/format tags survive the rename.
fn siblings_for(
    media_source: &Path,
    media_destination: &Path,
    policy: &ExtraFileImport,
) -> std::result::Result<Vec<(PathBuf, PathBuf)>, FsError> {
    let dir = match media_source.parent() {
        Some(d) => d,
        None => return Ok(Vec::new()),
    };
    let src_stem = match media_source.file_stem().and_then(|s| s.to_str()) {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(Vec::new()),
    };
    let dest_dir = media_destination.parent().unwrap_or_else(|| Path::new("."));
    let dest_stem = media_destination
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(src_stem);

    let mut pairs = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        // The source dir vanished (a single-file resume): nothing to import.
        Err(_) => return Ok(Vec::new()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // The media file itself is not its own extra.
        if path == *media_source {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let Some(suffix) = matching_suffix(name, src_stem, policy) else {
            continue;
        };
        // `suffix` includes the matched extension (e.g. ".en.srt" or ".srt").
        let dest_name = format!("{dest_stem}{suffix}");
        pairs.push((path.clone(), dest_dir.join(dest_name)));
    }
    // Deterministic order so a multi-extra import is reproducible.
    pairs.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(pairs)
}

/// The characters that may delimit the stem from a sibling's language/format tag.
/// A release writes subtitles as `Movie.en.srt`, `Movie - ENG.srt`, `Movie-eng.srt`
/// or `Movie_en.srt`; all are the same movie's subtitle. The boundary must be one of
/// these (not an alphanumeric) so `Show.S01E01` never matches `Show.S01E011.srt`
/// (a different episode).
const EXTRA_BOUNDARY: [char; 4] = ['.', '-', '_', ' '];

/// If `name` is an extra sibling of `stem` with a configured extension, return the
/// portion of the name *after* the stem (the language/format tag plus the
/// extension, e.g. `.en.srt` or ` - ENG.srt`); otherwise `None`.
///
/// The boundary after the stem must be a separator (see [`EXTRA_BOUNDARY`]) so a
/// common ` - ENG.srt`/`-eng.srt` naming is matched while `Show.S01E011.srt` (a
/// different episode) is not.
fn matching_suffix(name: &str, stem: &str, policy: &ExtraFileImport) -> Option<String> {
    let rest = name.strip_prefix(stem)?;
    if !rest.starts_with(EXTRA_BOUNDARY) {
        return None;
    }
    // The matched extension is everything after the final dot.
    let ext = rest.rsplit('.').next().unwrap_or("");
    if ext.is_empty() || !policy.matches_extension(ext) {
        return None;
    }
    Some(rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> ExtraFileImport {
        ExtraFileImport {
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn matching_suffix_preserves_language_tag() {
        let p = policy();
        assert_eq!(
            matching_suffix("Show.S01E01.en.srt", "Show.S01E01", &p),
            Some(".en.srt".to_string())
        );
        assert_eq!(
            matching_suffix("Show.S01E01.srt", "Show.S01E01", &p),
            Some(".srt".to_string())
        );
    }

    #[test]
    fn matching_suffix_accepts_dash_space_underscore_separators() {
        let p = policy();
        let stem = "Carnival.of.Souls.1962.BRRip.x264-Classics";
        // The common " - ENG.srt" form many releases use (the live Carnival case).
        assert_eq!(
            matching_suffix(&format!("{stem} - ENG.srt"), stem, &p),
            Some(" - ENG.srt".to_string())
        );
        // Dash- and underscore-delimited language tags.
        assert_eq!(
            matching_suffix(&format!("{stem}-eng.srt"), stem, &p),
            Some("-eng.srt".to_string())
        );
        assert_eq!(
            matching_suffix(&format!("{stem}_en.srt"), stem, &p),
            Some("_en.srt".to_string())
        );
    }

    #[test]
    fn matching_suffix_rejects_non_extras_and_episode_bleed() {
        let p = policy();
        // Different episode: the boundary after the stem is a digit, not a separator.
        assert_eq!(matching_suffix("Show.S01E011.srt", "Show.S01E01", &p), None);
        // A broadened separator must not let a different episode bleed in either.
        assert_eq!(matching_suffix("Show.S01E01x.srt", "Show.S01E01", &p), None);
        // The media file itself.
        assert_eq!(matching_suffix("Show.S01E01.mkv", "Show.S01E01", &p), None);
        // Unconfigured extension (even with a valid separator).
        assert_eq!(
            matching_suffix("Show.S01E01 - notes.txt", "Show.S01E01", &p),
            None
        );
    }

    #[tokio::test]
    async fn disabled_policy_imports_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("Movie.2021.mkv");
        std::fs::write(&src, b"x").unwrap();
        std::fs::write(dir.path().join("Movie.2021.en.srt"), b"s").unwrap();
        let disabled = ExtraFileImport {
            enabled: false,
            ..Default::default()
        };
        let out = import_extras(&src, dir.path().join("dest/Movie (2021).mkv"), &disabled).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn imports_sibling_srt_next_to_renamed_media() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("download");
        std::fs::create_dir_all(&src_dir).unwrap();
        let src = src_dir.join("Movie.2021.1080p.mkv");
        std::fs::write(&src, b"media").unwrap();
        std::fs::write(src_dir.join("Movie.2021.1080p.en.srt"), b"subs").unwrap();
        std::fs::write(src_dir.join("Movie.2021.1080p.idx"), b"idx").unwrap();
        // An unrelated file must be ignored.
        std::fs::write(src_dir.join("readme.txt"), b"hi").unwrap();

        let dest = dir.path().join("library/Movie (2021)/Movie (2021).mkv");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"media").unwrap();

        let out = import_extras(&src, &dest, &policy()).await;
        assert_eq!(out.len(), 2, "{out:?}");
        let dests: Vec<_> = out
            .iter()
            .filter_map(|o| match o {
                ExtraOutcome::Imported { destination, .. } => Some(
                    destination
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                ),
                _ => None,
            })
            .collect();
        assert!(
            dests.contains(&"Movie (2021).en.srt".to_string()),
            "{dests:?}"
        );
        assert!(dests.contains(&"Movie (2021).idx".to_string()), "{dests:?}");
        // The renamed subtitle exists on disk with the right content.
        let placed = dest.parent().unwrap().join("Movie (2021).en.srt");
        assert_eq!(std::fs::read(placed).unwrap(), b"subs");
    }

    #[tokio::test]
    async fn already_present_identical_extra_is_skipped_not_failed() {
        // A re-import where the subtitle is already correctly placed at its
        // destination (same content) must be a benign Skip, never a Failed — this
        // is the noise the reconcile adopt path surfaced.
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("download");
        std::fs::create_dir_all(&src_dir).unwrap();
        let src = src_dir.join("Movie.2021.1080p.mkv");
        std::fs::write(&src, b"media").unwrap();
        std::fs::write(src_dir.join("Movie.2021.1080p.en.srt"), b"subs").unwrap();

        let dest_dir = dir.path().join("library/Movie (2021)");
        std::fs::create_dir_all(&dest_dir).unwrap();
        let dest = dest_dir.join("Movie (2021).mkv");
        std::fs::write(&dest, b"media").unwrap();
        // The subtitle already sits at its destination with identical content.
        let placed = dest_dir.join("Movie (2021).en.srt");
        std::fs::write(&placed, b"subs").unwrap();

        let out = import_extras(&src, &dest, &policy()).await;
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(
            out[0],
            ExtraOutcome::Skipped {
                destination: placed.clone(),
                identical: true,
            },
            "an already-present identical subtitle is skipped, not failed"
        );
        // Untouched.
        assert_eq!(std::fs::read(&placed).unwrap(), b"subs");
    }

    #[tokio::test]
    async fn adopt_in_place_skips_its_own_sibling_subtitle() {
        // Adopt-in-place: the media source IS the library file, so its sibling
        // subtitle is already correctly named beside it. Importing must not try to
        // place the subtitle onto itself and report a failure — it is skipped.
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("library/Show/Season 01");
        std::fs::create_dir_all(&lib).unwrap();
        let media = lib.join("Show - S01E01.mkv");
        std::fs::write(&media, b"media").unwrap();
        let sub = lib.join("Show - S01E01.en.srt");
        std::fs::write(&sub, b"subs").unwrap();

        // source == destination (adopt-in-place).
        let out = import_extras(&media, &media, &policy()).await;
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(
            matches!(&out[0], ExtraOutcome::Skipped { destination, identical: true } if *destination == sub),
            "{out:?}"
        );
        assert_eq!(std::fs::read(&sub).unwrap(), b"subs", "left in place");
    }

    #[tokio::test]
    async fn a_different_file_at_the_destination_is_never_clobbered() {
        // Safety: if a *different* file already occupies the subtitle's target name,
        // it is left in place (not overwritten) and reported as a non-identical Skip.
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("download");
        std::fs::create_dir_all(&src_dir).unwrap();
        let src = src_dir.join("Movie.2021.1080p.mkv");
        std::fs::write(&src, b"media").unwrap();
        std::fs::write(src_dir.join("Movie.2021.1080p.en.srt"), b"new subtitle").unwrap();

        let dest_dir = dir.path().join("library/Movie (2021)");
        std::fs::create_dir_all(&dest_dir).unwrap();
        let dest = dest_dir.join("Movie (2021).mkv");
        std::fs::write(&dest, b"media").unwrap();
        let placed = dest_dir.join("Movie (2021).en.srt");
        std::fs::write(&placed, b"a pre-existing, different subtitle").unwrap();

        let out = import_extras(&src, &dest, &policy()).await;
        assert_eq!(
            out[0],
            ExtraOutcome::Skipped {
                destination: placed.clone(),
                identical: false,
            },
            "{out:?}"
        );
        assert_eq!(
            std::fs::read(&placed).unwrap(),
            b"a pre-existing, different subtitle",
            "the existing file must never be clobbered"
        );
    }

    #[tokio::test]
    async fn a_failing_extra_does_not_break_the_others() {
        // Point the destination directory at a path that cannot be created (a file
        // sits where the dir must be) and confirm we get a Failed outcome, never a
        // panic/propagated error.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("Movie.mkv");
        std::fs::write(&src, b"x").unwrap();
        std::fs::write(dir.path().join("Movie.srt"), b"s").unwrap();

        // A regular file occupies "blocked", so creating "blocked/Movie.*" fails.
        let blocker = dir.path().join("blocked");
        std::fs::write(&blocker, b"file-not-dir").unwrap();
        let dest = blocker.join("Movie (2021).mkv");

        let out = import_extras(&src, &dest, &policy()).await;
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], ExtraOutcome::Failed { .. }), "{out:?}");
    }
}
