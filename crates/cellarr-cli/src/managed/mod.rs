//! Config-as-code: reconcile cellarr's database from a declarative file.
//!
//! cellarr can keep its whole operational configuration — tags, root folders,
//! libraries, quality definitions, custom formats, quality profiles, indexers,
//! download clients — in a single declarative YAML file that lives in git next to
//! the deployment (a k8s ConfigMap). On boot, if a managed-config path is
//! configured, the daemon **reconciles** the DB to match the file: it diffs the
//! declared state against what config previously managed (a tracking ledger) and
//! creates / updates / prunes through the existing repos, leaving UI-created
//! entities untouched. A whole section absent from the file is left alone.
//!
//! The pieces:
//!
//! - [`schema`] — the typed [`ManagedConfig`] the YAML deserializes into, mirroring
//!   the `/api/v3` + core models so it feels familiar.
//! - [`interpolate`] — `${ENV}` / `${ENV:-default}` secret resolution on the raw
//!   text, so committed config never contains a key/password.
//! - [`loader`] — read → interpolate → parse → version-check → validate.
//! - [`validate`] — cross-reference + uniqueness checks (broken refs fail loudly).
//! - [`plan`] — the **pure** diff step (declared vs ledger → create/update/prune).
//! - [`reconcile`] — the apply step (and the read-only dry-run plan).
//! - [`export`] — dump the live DB state as a round-trippable [`ManagedConfig`].
//!
//! [`ManagedConfig`]: schema::ManagedConfig

pub mod error;
pub mod export;
pub mod interpolate;
pub mod loader;
pub mod plan;
pub mod reconcile;
pub mod schema;
pub mod validate;

use std::path::Path;

use cellarr_db::Database;
use tracing::info;

pub use error::ManagedError;
pub use reconcile::ReconcileReport;
pub use schema::ManagedConfig;

/// Load and apply the managed config at `path` to `db`, logging a one-line summary
/// per kind (created/updated/pruned/unchanged). This is the boot entry point.
///
/// A failure here must fail boot: a half-applied or stale config is worse than not
/// serving, so the caller propagates the error rather than degrading.
///
/// # Errors
/// Returns a [`ManagedError`] for any load, validation, or apply failure.
pub async fn reconcile_on_boot(
    db: &Database,
    path: &Path,
) -> Result<ReconcileReport, ManagedError> {
    let config = loader::load(path)?;
    let report = reconcile::apply(db, &config).await?;
    for kind in &report.kinds {
        let c = kind.counts();
        info!(
            target: "cellarr::managed",
            kind = kind.kind,
            created = c.created,
            updated = c.updated,
            pruned = c.pruned,
            unchanged = c.unchanged,
            "managed config reconciled"
        );
    }
    Ok(report)
}

/// Render a [`ReconcileReport`] as a human-readable diff (the `config validate`
/// output). One block per kind, one line per changed item; unchanged items are
/// summarized as a count so the diff stays focused on what would move.
#[must_use]
pub fn render_diff(report: &ReconcileReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    if report.kinds.is_empty() {
        return "no managed sections declared (nothing to reconcile)\n".to_string();
    }
    for kind in &report.kinds {
        let c = kind.counts();
        let _ = writeln!(
            out,
            "{}: {} create, {} update, {} prune, {} unchanged",
            kind.kind, c.created, c.updated, c.pruned, c.unchanged
        );
        for item in &kind.items {
            if item.action.is_change() {
                let _ = writeln!(out, "  {:>9} {}", item.action.label(), item.name);
            }
        }
    }
    let total = report.totals();
    let _ = writeln!(
        out,
        "total: {} create, {} update, {} prune, {} unchanged",
        total.created, total.updated, total.pruned, total.unchanged
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_diff_of_empty_report() {
        let report = ReconcileReport::default();
        assert!(render_diff(&report).contains("nothing to reconcile"));
    }
}
