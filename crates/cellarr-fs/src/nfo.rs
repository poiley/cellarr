//! Kodi/Jellyfin-compatible `.nfo` metadata sidecar export.
//!
//! On import, media servers (Kodi, Jellyfin, Emby) read a small XML sidecar next
//! to the media to display title/year/overview/air-date without re-scraping. This
//! module renders those sidecars and places them per the well-known convention:
//!
//! - **movie**: `movie.nfo` in the movie's folder (next to the film file);
//! - **series**: `tvshow.nfo` in the series root folder;
//! - **episode**: `<episode-file-basename>.nfo` next to the episode file.
//!
//! ### Why this is crash-safe
//! The sidecar is a *derived, regenerable* file, never the user's media. It is
//! written **after** the media files are durably committed by
//! [`execute_import`](crate::execute_import), as a separate best-effort step, so a
//! failure (or crash) writing an `.nfo` can never affect the stage→verify→commit
//! guarantee for the media itself: the worst case is a missing sidecar, which a
//! re-run or a metadata refresh regenerates. Each sidecar is written atomically
//! (temp file + fsync + rename) so a partial `.nfo` is never observed.

use std::path::{Path, PathBuf};

use crate::error::{FsError, Result};
use crate::fsops;

/// The facts an `.nfo` sidecar carries. A field left `None`/empty is simply
/// omitted from the XML (Kodi/Jellyfin treat absent elements as unknown).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NfoMetadata {
    /// Display title.
    pub title: Option<String>,
    /// Release/first-air year.
    pub year: Option<u16>,
    /// Plot/overview.
    pub overview: Option<String>,
    /// Runtime in minutes.
    pub runtime: Option<u32>,
    /// Air date (episode) / release date (movie), ISO `yyyy-mm-dd`.
    pub air_date: Option<String>,
    /// Season number (episode sidecars only).
    pub season: Option<u32>,
    /// Episode number (episode sidecars only).
    pub episode: Option<u32>,
}

/// The kind of sidecar to render (chooses the root element and which fields are
/// meaningful).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NfoKind {
    /// A `<movie>` sidecar (`movie.nfo`).
    Movie,
    /// A `<tvshow>` sidecar (`tvshow.nfo`).
    Series,
    /// An `<episodedetails>` sidecar (`<file>.nfo`).
    Episode,
}

/// Escape a string for XML text content (`&`, `<`, `>`). Quotes are not escaped
/// because the renderer never places text inside an attribute.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Append `<tag>escaped(value)</tag>\n` when `value` is present and non-empty.
fn push_text(out: &mut String, tag: &str, value: Option<&str>) {
    if let Some(v) = value.filter(|s| !s.is_empty()) {
        out.push_str(&format!("  <{tag}>{}</{tag}>\n", escape(v)));
    }
}

/// Append `<tag>n</tag>\n` when `value` is present.
fn push_num(out: &mut String, tag: &str, value: Option<u64>) {
    if let Some(n) = value {
        out.push_str(&format!("  <{tag}>{n}</{tag}>\n"));
    }
}

/// Render an `.nfo` document for `kind` from `meta`. The output is a complete,
/// well-formed XML document (declaration + root element) using the element names
/// Kodi/Jellyfin read.
#[must_use]
pub fn render_nfo(kind: NfoKind, meta: &NfoMetadata) -> String {
    let root = match kind {
        NfoKind::Movie => "movie",
        NfoKind::Series => "tvshow",
        NfoKind::Episode => "episodedetails",
    };
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    out.push_str(&format!("<{root}>\n"));
    push_text(&mut out, "title", meta.title.as_deref());
    push_text(&mut out, "plot", meta.overview.as_deref());
    push_num(&mut out, "year", meta.year.map(u64::from));
    push_num(&mut out, "runtime", meta.runtime.map(u64::from));
    // The release/air date element differs by kind: movies use <premiered>,
    // episodes use <aired> (both ISO yyyy-mm-dd) — the spellings Kodi expects.
    match kind {
        NfoKind::Movie | NfoKind::Series => {
            push_text(&mut out, "premiered", meta.air_date.as_deref());
        }
        NfoKind::Episode => {
            push_text(&mut out, "aired", meta.air_date.as_deref());
            push_num(&mut out, "season", meta.season.map(u64::from));
            push_num(&mut out, "episode", meta.episode.map(u64::from));
        }
    }
    out.push_str(&format!("</{root}>\n"));
    out
}

/// The sidecar path for a committed media file of `kind`:
/// - [`NfoKind::Movie`] → `movie.nfo` in the file's parent dir;
/// - [`NfoKind::Series`] → `tvshow.nfo` in the file's parent dir;
/// - [`NfoKind::Episode`] → `<file-stem>.nfo` beside the file.
///
/// Returns `None` only when `media_file` has no parent (a bare filename with no
/// directory) — there is then nowhere to place a sidecar.
#[must_use]
pub fn sidecar_path(kind: NfoKind, media_file: &Path) -> Option<PathBuf> {
    let parent = media_file.parent()?;
    let name = match kind {
        NfoKind::Movie => "movie.nfo".to_string(),
        NfoKind::Series => "tvshow.nfo".to_string(),
        NfoKind::Episode => {
            let stem = media_file.file_stem()?.to_string_lossy();
            format!("{stem}.nfo")
        }
    };
    Some(parent.join(name))
}

/// Write the `.nfo` sidecar for a committed media file, atomically and durably.
///
/// This is the best-effort post-commit step: it is called only after the media
/// file is durable, and writes the sidecar via a temp file + fsync + atomic
/// rename so a partial `.nfo` is never observed. Overwrites any existing sidecar
/// (a refresh regenerates it).
///
/// # Errors
/// Returns an [`FsError`] if the sidecar cannot be written. Callers treat this as
/// non-fatal (a missing sidecar is regenerable) and log rather than failing the
/// import.
pub async fn write_sidecar(kind: NfoKind, media_file: &Path, meta: &NfoMetadata) -> Result<()> {
    let Some(dest) = sidecar_path(kind, media_file) else {
        return Err(FsError::MissingPath {
            path: media_file.to_path_buf(),
        });
    };
    let body = render_nfo(kind, meta);
    write_atomic(&dest, body.as_bytes()).await
}

/// Write `bytes` to `dest` atomically: a sibling temp file, fsync'd, then renamed
/// over the destination. Mirrors the import commit's durability so a crash never
/// leaves a half-written sidecar.
async fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fsops::create_dir_all(parent).await?;
    }
    let tmp = staging_path(dest);
    let tmp_clone = tmp.clone();
    let dest_clone = dest.to_path_buf();
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || -> Result<()> {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp_clone).map_err(|e| FsError::io(&tmp_clone, e))?;
        f.write_all(&bytes)
            .map_err(|e| FsError::io(&tmp_clone, e))?;
        f.sync_all().map_err(|e| FsError::io(&tmp_clone, e))?;
        std::fs::rename(&tmp_clone, &dest_clone).map_err(|e| FsError::io(&dest_clone, e))?;
        Ok(())
    })
    .await
    .map_err(|e| FsError::TaskJoin(e.to_string()))?
    .inspect_err(|_| {
        // Best-effort cleanup of the temp file if the rename failed.
        let _ = std::fs::remove_file(&tmp);
    })
}

/// A unique staging path beside `dest` for the atomic write.
fn staging_path(dest: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = dest
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let staged = format!(".cellarr-nfo.{}.{}.{}", std::process::id(), n, name);
    dest.parent()
        .map(|p| p.join(&staged))
        .unwrap_or_else(|| PathBuf::from(staged))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movie_nfo_has_expected_elements() {
        let meta = NfoMetadata {
            title: Some("Blade Runner".into()),
            year: Some(1982),
            overview: Some("A blade runner must pursue replicants.".into()),
            runtime: Some(117),
            air_date: Some("1982-06-25".into()),
            ..Default::default()
        };
        let xml = render_nfo(NfoKind::Movie, &meta);
        assert!(xml.starts_with("<?xml version=\"1.0\""));
        assert!(xml.contains("<movie>"));
        assert!(xml.contains("<title>Blade Runner</title>"));
        assert!(xml.contains("<year>1982</year>"));
        assert!(xml.contains("<runtime>117</runtime>"));
        assert!(xml.contains("<premiered>1982-06-25</premiered>"));
        assert!(xml.trim_end().ends_with("</movie>"));
    }

    #[test]
    fn episode_nfo_uses_aired_and_numbering() {
        let meta = NfoMetadata {
            title: Some("Pilot".into()),
            air_date: Some("2008-01-20".into()),
            season: Some(1),
            episode: Some(1),
            ..Default::default()
        };
        let xml = render_nfo(NfoKind::Episode, &meta);
        assert!(xml.contains("<episodedetails>"));
        assert!(xml.contains("<aired>2008-01-20</aired>"));
        assert!(xml.contains("<season>1</season>"));
        assert!(xml.contains("<episode>1</episode>"));
        assert!(!xml.contains("<premiered>"));
    }

    #[test]
    fn escapes_xml_special_characters() {
        let meta = NfoMetadata {
            title: Some("Tom & Jerry <2021>".into()),
            ..Default::default()
        };
        let xml = render_nfo(NfoKind::Movie, &meta);
        assert!(xml.contains("<title>Tom &amp; Jerry &lt;2021&gt;</title>"));
    }

    #[test]
    fn absent_fields_are_omitted() {
        let xml = render_nfo(NfoKind::Movie, &NfoMetadata::default());
        assert!(!xml.contains("<title>"));
        assert!(!xml.contains("<year>"));
        assert!(xml.contains("<movie>"));
    }

    #[test]
    fn sidecar_paths_follow_convention() {
        let movie = Path::new("/lib/Blade Runner (1982)/Blade Runner (1982).mkv");
        assert_eq!(
            sidecar_path(NfoKind::Movie, movie).unwrap(),
            PathBuf::from("/lib/Blade Runner (1982)/movie.nfo")
        );
        let ep = Path::new("/lib/Show/Season 01/Show - S01E01.mkv");
        assert_eq!(
            sidecar_path(NfoKind::Episode, ep).unwrap(),
            PathBuf::from("/lib/Show/Season 01/Show - S01E01.nfo")
        );
        assert_eq!(
            sidecar_path(NfoKind::Series, ep).unwrap(),
            PathBuf::from("/lib/Show/Season 01/tvshow.nfo")
        );
    }
}
