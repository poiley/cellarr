//! The system health checks surfaced on `/api/v3/health`.
//!
//! Each check runs against the live config/filesystem/database and yields a
//! [`HealthCheck`] record `{ source, type, message, wikiUrl }` — the exact shape
//! the *arr ecosystem (and cellarr's own UI) reads. An all-clear check yields
//! nothing; the v3 health array is the union of every check's findings.
//!
//! The checks mirror the originals' health surface so dashboards light up the
//! same way:
//! - **no-root-folder** / **root-folder-unwritable** — nothing to import into, or
//!   a configured root that cannot be written (a real local write-probe);
//! - **no-indexer** / **indexer-unreachable** — nothing to search, or a configured
//!   indexer that fails its connectivity test;
//! - **no-download-client** / **download-client-unreachable** — nowhere to send a
//!   grab, or a configured client that is unreachable;
//! - **no-recent-backup** — no backup taken within the freshness window;
//! - **database-ok** — a liveness probe of the persistence layer.
//!
//! The *-unreachable network probes are gated behind a live reachability seam the
//! daemon may inject; with none (the offline/test default) those checks are
//! skipped rather than guessed. The structural and local-filesystem checks always
//! run.

use cellarr_db::Database;
use serde::Serialize;
use serde_json::{json, Value};

use crate::backup::BackupEngine;
use crate::error::ApiResult;

/// The severity of a health finding, matching the v3 `type` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// An advisory: degraded but functional.
    Warning,
    /// A fault that blocks core function (e.g. no root folder to import into).
    Error,
}

impl Severity {
    fn as_str(self) -> &'static str {
        match self {
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// One health finding.
#[derive(Debug, Clone, Serialize)]
pub struct HealthCheck {
    /// The check source identifier (e.g. `RootFolderCheck`), matching the v3
    /// `source` field the ecosystem groups on.
    pub source: &'static str,
    /// Severity.
    pub severity: Severity,
    /// Human-readable message.
    pub message: String,
    /// A stable "type" key (a wiki-ish slug like `no-root-folder`) so a client can
    /// branch on the specific condition without parsing the message.
    pub check_type: &'static str,
}

impl HealthCheck {
    /// Render this finding into the v3 `{ source, type, message, wikiUrl }` shape.
    /// `wikiUrl` carries the cellarr docs anchor for the check type.
    #[must_use]
    pub fn to_v3(&self) -> Value {
        json!({
            "source": self.source,
            "type": self.severity.as_str(),
            "message": self.message,
            "wikiUrl": format!("https://cellarr.invalid/docs/health#{}", self.check_type),
        })
    }
}

/// How recent a backup must be before `no-recent-backup` clears (7 days).
pub const BACKUP_FRESHNESS_SECS: i64 = 7 * 24 * 60 * 60;

/// Run every health check and collect the findings.
///
/// `backup` enables the `no-recent-backup` check when supplied. The
/// cross-filesystem warning ([`crate::fs_health`]) is folded in by the caller (it
/// already has its own dedicated reporting), so this focuses on the structural,
/// writability, freshness, and liveness checks.
///
/// # Errors
/// Propagates a persistence error if the config or a liveness probe fails to run.
pub async fn run_all(db: &Database, backup: Option<&BackupEngine>) -> ApiResult<Vec<HealthCheck>> {
    let mut out = Vec::new();
    let cfg = db.config();

    // --- database-ok: a liveness probe of the persistence layer ---------------
    // A trivial read confirms the pool is live and the schema is queryable. A
    // failure here is an error finding (the rest of the daemon cannot function).
    match cfg.list_libraries().await {
        Ok(_) => {}
        Err(e) => out.push(HealthCheck {
            source: "DatabaseCheck",
            severity: Severity::Error,
            message: format!("Database is not responding: {e}"),
            check_type: "database-unhealthy",
        }),
    }

    // --- root folders ---------------------------------------------------------
    let libraries = cfg.list_libraries().await?;
    let standalone = cfg.list_root_folders().await?;
    let mut roots: Vec<String> = Vec::new();
    for lib in &libraries {
        for r in &lib.root_folders {
            if !roots.contains(r) {
                roots.push(r.clone());
            }
        }
    }
    for rf in &standalone {
        if !roots.contains(&rf.path) {
            roots.push(rf.path.clone());
        }
    }
    if roots.is_empty() {
        out.push(HealthCheck {
            source: "RootFolderCheck",
            severity: Severity::Error,
            message: "No root folders are configured".into(),
            check_type: "no-root-folder",
        });
    } else {
        for root in &roots {
            if let Some(reason) = unwritable_reason(root) {
                out.push(HealthCheck {
                    source: "RootFolderCheck",
                    severity: Severity::Error,
                    message: format!("Root folder '{root}' is not writable: {reason}"),
                    check_type: "root-folder-unwritable",
                });
            }
        }
    }

    // --- indexers -------------------------------------------------------------
    if cfg.list_indexers().await?.is_empty() {
        out.push(HealthCheck {
            source: "IndexerCheck",
            severity: Severity::Warning,
            message: "No indexers are configured".into(),
            check_type: "no-indexer",
        });
    }
    // indexer-unreachable: a live connectivity probe per configured indexer.
    // TODO(reachability-probe): wire a reachability seam (the same one the
    // `indexer/test` route will use once live) and emit an
    // `indexer-unreachable` warning per indexer that fails its test. Skipped here
    // because the offline/default build has no network seam, and a guessed probe
    // would either false-positive offline or violate the offline non-negotiable.

    // --- download clients -----------------------------------------------------
    if cfg.list_download_clients().await?.is_empty() {
        out.push(HealthCheck {
            source: "DownloadClientCheck",
            severity: Severity::Warning,
            message: "No download client is configured".into(),
            check_type: "no-download-client",
        });
    }
    // download-client-unreachable: same deferral as indexer-unreachable above.
    // TODO(reachability-probe): emit a `download-client-unreachable` warning per
    // client that fails its connectivity test once the live seam is wired.

    // --- no-recent-backup -----------------------------------------------------
    if let Some(engine) = backup {
        let newest = engine
            .list()
            .ok()
            .and_then(|b| b.iter().map(|i| i.created_unix).max());
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let stale = match newest {
            None => true,
            Some(ts) => now.saturating_sub(ts) > BACKUP_FRESHNESS_SECS,
        };
        if stale {
            out.push(HealthCheck {
                source: "BackupCheck",
                severity: Severity::Warning,
                message: "No recent backup of the database was found".into(),
                check_type: "no-recent-backup",
            });
        }
    }

    Ok(out)
}

/// Probe whether `path` is a writable directory, returning `Some(reason)` when it
/// is not (missing, not a directory, or a failed write probe). `None` means
/// writable. The probe creates and removes a uniquely-named temp file so it never
/// disturbs library contents.
fn unwritable_reason(path: &str) -> Option<String> {
    let p = std::path::Path::new(path);
    let meta = match std::fs::metadata(p) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Some("path does not exist".into())
        }
        Err(e) => return Some(format!("cannot stat: {e}")),
    };
    if !meta.is_dir() {
        return Some("path is not a directory".into());
    }
    // Write probe: a uniquely-named dotfile we immediately remove.
    let probe = p.join(format!(
        ".cellarr-write-probe-{}",
        time::OffsetDateTime::now_utc().unix_timestamp_nanos()
    ));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            None
        }
        Err(e) => Some(format!("write probe failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A file-backed temp DB: the in-memory pool pins its single connection to the
    // writer-actor, so concurrent repo reads (which health does) time out — a
    // file-backed pool has the headroom the real daemon does.
    async fn temp_db(dir: &std::path::Path) -> Database {
        Database::open(dir.join("cellarr.sqlite").to_str().unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn db_ok_and_no_root_no_client_on_empty_config() {
        let dir = tempfile::tempdir().unwrap();
        let db = temp_db(dir.path()).await;
        let checks = run_all(&db, None).await.unwrap();
        // database-ok: NO DatabaseCheck finding means the probe passed.
        assert!(!checks.iter().any(|c| c.source == "DatabaseCheck"));
        // Missing root folder is an error.
        let root = checks.iter().find(|c| c.check_type == "no-root-folder");
        assert!(root.is_some());
        assert_eq!(root.unwrap().severity, Severity::Error);
        // No download client / indexer are warnings.
        assert!(checks
            .iter()
            .any(|c| c.check_type == "no-download-client" && c.severity == Severity::Warning));
        assert!(checks
            .iter()
            .any(|c| c.check_type == "no-indexer" && c.severity == Severity::Warning));
        db.shutdown().await;
    }

    #[tokio::test]
    async fn writable_root_clears_no_root_and_unwritable() {
        let dir = tempfile::tempdir().unwrap();
        let db = temp_db(dir.path()).await;
        let rf = cellarr_core::RootFolder {
            id: uuid::Uuid::new_v4().to_string(),
            path: dir.path().to_str().unwrap().to_string(),
            name: None,
            enabled: true,
        };
        db.config().upsert_root_folder(&rf).await.unwrap();
        let checks = run_all(&db, None).await.unwrap();
        assert!(!checks.iter().any(|c| c.check_type == "no-root-folder"));
        assert!(!checks
            .iter()
            .any(|c| c.check_type == "root-folder-unwritable"));
        db.shutdown().await;
    }

    #[tokio::test]
    async fn missing_root_path_is_unwritable() {
        let dir = tempfile::tempdir().unwrap();
        let db = temp_db(dir.path()).await;
        let rf = cellarr_core::RootFolder {
            id: uuid::Uuid::new_v4().to_string(),
            path: "/this/path/does/not/exist/at/all".into(),
            name: None,
            enabled: true,
        };
        db.config().upsert_root_folder(&rf).await.unwrap();
        let checks = run_all(&db, None).await.unwrap();
        let unwritable = checks
            .iter()
            .find(|c| c.check_type == "root-folder-unwritable");
        assert!(unwritable.is_some());
        assert_eq!(unwritable.unwrap().severity, Severity::Error);
        db.shutdown().await;
    }

    #[test]
    fn v3_shape_has_expected_fields() {
        let c = HealthCheck {
            source: "RootFolderCheck",
            severity: Severity::Error,
            message: "No root folders are configured".into(),
            check_type: "no-root-folder",
        };
        let v = c.to_v3();
        assert_eq!(v["source"], "RootFolderCheck");
        assert_eq!(v["type"], "error");
        assert_eq!(v["message"], "No root folders are configured");
        assert!(v["wikiUrl"].as_str().unwrap().contains("no-root-folder"));
    }
}
