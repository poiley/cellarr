//! Log-file listing + tailing for the `/api/v3/log/file` surface.
//!
//! Mirrors the Sonarr/Radarr `log/file` resources: the operator can list the
//! daemon's on-disk log files and read the tail of one from the System → Log
//! Files screen, without shell access to the host. The daemon writes a rolling
//! log file under `<data_dir>/logs` (the CLI installs the rolling file appender);
//! this module reads those files back.
//!
//! ## Safety
//! The file name in `GET /api/v3/log/file/{name}` is **untrusted**. [`read_tail`]
//! rejects any name containing a path separator, `..`, or a NUL, and resolves the
//! file strictly inside the configured logs directory — so a crafted name can
//! never read `/etc/passwd` or escape the logs dir. Only regular files with a
//! `.log`/`.txt` extension are listed/served. Nothing beyond what is already in
//! the log lines is redacted (secrets are never logged in the first place — keys
//! are kept out of logs at the source, per the config module).

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Errors from the log-file surface.
#[derive(Debug, thiserror::Error)]
pub enum LogFileError {
    /// An I/O error reading the logs directory or a log file.
    #[error("log io error: {0}")]
    Io(String),
    /// The requested log file name is invalid (traversal attempt) or absent.
    #[error("log file {0} not found")]
    NotFound(String),
}

type Result<T> = std::result::Result<T, LogFileError>;

/// A listed log file.
#[derive(Debug, Clone, Serialize)]
pub struct LogFileInfo {
    /// The bare file name (what `GET /log/file/{name}` takes).
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Last-modified time (unix seconds), best-effort.
    pub last_modified_unix: i64,
}

/// The default cap on lines returned by a tail read when the caller gives none.
pub const DEFAULT_TAIL_LINES: usize = 500;
/// The hard cap on lines a single tail read returns, regardless of the request.
pub const MAX_TAIL_LINES: usize = 10_000;

/// The log-file reader, bound to the daemon's logs directory.
#[derive(Clone)]
pub struct LogFiles {
    dir: PathBuf,
}

impl LogFiles {
    /// Bind to the logs directory.
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The logs directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// List the log files, newest-modified first.
    ///
    /// # Errors
    /// Returns a [`LogFileError::Io`] if the directory exists but cannot be read.
    /// A missing directory is not an error (no logs yet → empty list).
    pub fn list(&self) -> Result<Vec<LogFileInfo>> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(LogFileError::Io(format!("reading logs dir: {e}"))),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_log_file(&path) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let meta = match entry.metadata() {
                Ok(m) if m.is_file() => m,
                _ => continue,
            };
            out.push(LogFileInfo {
                name: name.to_string(),
                size: meta.len(),
                last_modified_unix: mtime_unix(&meta),
            });
        }
        out.sort_by(|a, b| {
            b.last_modified_unix
                .cmp(&a.last_modified_unix)
                .then(a.name.cmp(&b.name))
        });
        Ok(out)
    }

    /// Read the last `limit` lines of the named log file (defaulting to
    /// [`DEFAULT_TAIL_LINES`], capped at [`MAX_TAIL_LINES`]).
    ///
    /// The `name` is validated against traversal: it must be a bare file name with
    /// a log extension, resolving strictly inside the logs directory.
    ///
    /// # Errors
    /// [`LogFileError::NotFound`] if the name is invalid or the file is absent;
    /// [`LogFileError::Io`] if a present file cannot be read.
    pub fn read_tail(&self, name: &str, limit: Option<usize>) -> Result<Vec<String>> {
        let path = self.resolve(name)?;
        let content =
            std::fs::read_to_string(&path).map_err(|_| LogFileError::NotFound(name.to_string()))?;
        let limit = limit.unwrap_or(DEFAULT_TAIL_LINES).clamp(1, MAX_TAIL_LINES);
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(limit);
        Ok(lines[start..].iter().map(|s| (*s).to_string()).collect())
    }

    /// Resolve an untrusted file name to a path strictly inside the logs dir,
    /// rejecting any traversal. The name must be a bare log file name.
    fn resolve(&self, name: &str) -> Result<PathBuf> {
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || name.contains("..")
            || name.contains('\0')
        {
            return Err(LogFileError::NotFound(name.to_string()));
        }
        let candidate = self.dir.join(name);
        // Defense in depth: the resolved path's parent must be exactly the logs
        // dir, and it must carry a log extension.
        if candidate.parent() != Some(self.dir.as_path()) || !is_log_file(&candidate) {
            return Err(LogFileError::NotFound(name.to_string()));
        }
        Ok(candidate)
    }
}

/// Whether `path`'s name carries a log extension we list/serve.
fn is_log_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("log") | Some("txt")
    ) || path
        // A rolling appender may suffix the date after `.log` (e.g.
        // `cellarr.log.2026-06-24`); accept those by checking the stem.
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.contains(".log"))
        .unwrap_or(false)
}

/// Best-effort last-modified time as unix seconds (0 if unavailable).
fn mtime_unix(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn lists_only_log_files() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "cellarr.log", "a\nb\n");
        write(dir.path(), "notes.md", "ignore me");
        write(dir.path(), "old.txt", "x");
        let lf = LogFiles::new(dir.path().to_path_buf());
        let names: Vec<_> = lf.list().unwrap().into_iter().map(|f| f.name).collect();
        assert!(names.contains(&"cellarr.log".to_string()));
        assert!(names.contains(&"old.txt".to_string()));
        assert!(!names.contains(&"notes.md".to_string()));
    }

    #[test]
    fn tail_returns_written_lines_and_respects_limit() {
        let dir = tempfile::tempdir().unwrap();
        let body = (1..=10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        write(dir.path(), "cellarr.log", &body);
        let lf = LogFiles::new(dir.path().to_path_buf());

        let all = lf.read_tail("cellarr.log", None).unwrap();
        assert_eq!(all.len(), 10);
        assert_eq!(all[0], "line 1");
        assert_eq!(all[9], "line 10");

        let last3 = lf.read_tail("cellarr.log", Some(3)).unwrap();
        assert_eq!(last3, vec!["line 8", "line 9", "line 10"]);

        // A zero limit is clamped to at least one line.
        let one = lf.read_tail("cellarr.log", Some(0)).unwrap();
        assert_eq!(one, vec!["line 10"]);
    }

    #[test]
    fn rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "cellarr.log", "secret-free\n");
        let lf = LogFiles::new(dir.path().to_path_buf());
        for bad in [
            "../../etc/passwd",
            "..",
            "a/b.log",
            "a\\b.log",
            "",
            "sub/../cellarr.log",
        ] {
            assert!(
                lf.read_tail(bad, None).is_err(),
                "should reject name {bad:?}"
            );
        }
        // A non-log extension is also refused even with no traversal.
        write(dir.path(), "secret.key", "TOPSECRET");
        assert!(lf.read_tail("secret.key", None).is_err());
    }
}
