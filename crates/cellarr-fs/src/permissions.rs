//! Post-commit permission application (chmod / chown).
//!
//! After the crash-safe import has durably committed the media, the user's
//! [`ImportPermissions`](cellarr_core::ImportPermissions) policy is applied to the
//! placed files and the directories created for them. This is **strictly
//! best-effort and Unix-only**: it runs *after* the media is durable, so a chmod
//! or chown failure can never corrupt, partial, or roll back the imported file —
//! the caller logs the failure and continues (the import already succeeded).
//!
//! On non-Unix platforms the whole step is a no-op (the `chmod`/`chown` model does
//! not apply), reported as [`PermissionOutcome::Unsupported`] so the caller can
//! note it without treating it as an error.
//!
//! The implementation uses only safe std APIs (`std::fs::set_permissions`,
//! `std::os::unix::fs::chown`); user/group *names* are resolved by reading
//! `/etc/passwd` / `/etc/group`, so the crate keeps its `#![forbid(unsafe_code)]`
//! guarantee and pulls in no extra dependency. Numeric ids are always honored;
//! names resolve when the standard databases are file-backed (the container case).

use std::path::{Path, PathBuf};

use cellarr_core::ImportPermissions;

use crate::error::FsError;

/// What happened when a permission policy was applied to one path.
///
/// Every variant is non-fatal: the import is already durable. `Failed` carries the
/// reason so the caller can log it; it never propagates as an import error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// The policy was applied (or there was nothing to apply).
    Applied {
        /// The path the policy targeted.
        path: PathBuf,
        /// Whether a chmod was actually performed.
        chmod: bool,
        /// Whether a chown was actually performed.
        chown: bool,
    },
    /// The platform does not support the chmod/chown model (non-Unix). A no-op.
    Unsupported {
        /// The path that would have been targeted.
        path: PathBuf,
    },
    /// Applying the policy failed. The import is unaffected; log and continue.
    Failed {
        /// The path the policy targeted.
        path: PathBuf,
        /// Why it failed (already formatted for a log line).
        reason: String,
    },
}

impl PermissionOutcome {
    /// Whether this outcome represents a failure the caller should log.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        matches!(self, PermissionOutcome::Failed { .. })
    }
}

/// Apply `perms` to an imported `file` path (the placed media) on a blocking
/// thread.
///
/// Never returns an error: the import is already durable, so a failure is folded
/// into [`PermissionOutcome::Failed`] for the caller to log, not propagated.
pub async fn apply_to_file(
    file: impl Into<PathBuf>,
    perms: &ImportPermissions,
) -> PermissionOutcome {
    apply(file, perms, perms.chmod_file.as_deref()).await
}

/// Apply `perms` to a `folder` path (a directory the import created). Uses the
/// folder chmod mode; otherwise identical to [`apply_to_file`].
pub async fn apply_to_folder(
    folder: impl Into<PathBuf>,
    perms: &ImportPermissions,
) -> PermissionOutcome {
    apply(folder, perms, perms.chmod_folder.as_deref()).await
}

/// Shared apply path: a `chmod_mode` (file or folder) plus the shared `chown`.
async fn apply(
    path: impl Into<PathBuf>,
    perms: &ImportPermissions,
    chmod_mode: Option<&str>,
) -> PermissionOutcome {
    let path = path.into();
    let chmod_mode = chmod_mode.map(str::to_string);
    let chown = perms.chown.clone();
    match tokio::task::spawn_blocking(move || {
        apply_blocking(&path, chmod_mode.as_deref(), chown.as_deref())
    })
    .await
    {
        Ok(outcome) => outcome,
        Err(e) => PermissionOutcome::Failed {
            path: PathBuf::new(),
            reason: FsError::TaskJoin(e.to_string()).to_string(),
        },
    }
}

#[cfg(unix)]
fn apply_blocking(path: &Path, chmod_mode: Option<&str>, chown: Option<&str>) -> PermissionOutcome {
    let mut did_chmod = false;
    let mut did_chown = false;

    if let Some(mode_str) = chmod_mode.map(str::trim).filter(|s| !s.is_empty()) {
        match parse_octal_mode(mode_str) {
            Some(mode) => {
                use std::os::unix::fs::PermissionsExt;
                let perm = std::fs::Permissions::from_mode(mode);
                if let Err(e) = std::fs::set_permissions(path, perm) {
                    return PermissionOutcome::Failed {
                        path: path.to_path_buf(),
                        reason: format!("chmod {mode_str} failed: {e}"),
                    };
                }
                did_chmod = true;
            }
            None => {
                return PermissionOutcome::Failed {
                    path: path.to_path_buf(),
                    reason: format!("invalid octal mode {mode_str:?}"),
                };
            }
        }
    }

    if let Some(spec) = chown.map(str::trim).filter(|s| !s.is_empty()) {
        match resolve_chown(spec) {
            Ok((uid, gid)) => {
                if let Err(e) = std::os::unix::fs::chown(path, uid, gid) {
                    return PermissionOutcome::Failed {
                        path: path.to_path_buf(),
                        reason: format!("chown {spec:?} failed: {e}"),
                    };
                }
                did_chown = true;
            }
            Err(reason) => {
                return PermissionOutcome::Failed {
                    path: path.to_path_buf(),
                    reason: format!("chown {spec:?}: {reason}"),
                };
            }
        }
    }

    PermissionOutcome::Applied {
        path: path.to_path_buf(),
        chmod: did_chmod,
        chown: did_chown,
    }
}

#[cfg(not(unix))]
fn apply_blocking(
    path: &Path,
    _chmod_mode: Option<&str>,
    _chown: Option<&str>,
) -> PermissionOutcome {
    // The chmod/chown model does not apply off Unix; report unsupported rather
    // than fail so the caller can note it without it counting as an error.
    PermissionOutcome::Unsupported {
        path: path.to_path_buf(),
    }
}

/// Parse an octal mode string (`"755"`, `"0644"`) into a `u32` mode, or `None`
/// when it is not a valid octal number.
fn parse_octal_mode(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches("0o");
    if s.is_empty() || !s.chars().all(|c| ('0'..='7').contains(&c)) {
        return None;
    }
    u32::from_str_radix(s, 8).ok()
}

/// Resolve a `user:group` (or `user`, or `:group`) chown spec into the optional
/// numeric ids `std::os::unix::fs::chown` expects (an omitted side is `None` =
/// "leave unchanged").
#[cfg(unix)]
fn resolve_chown(spec: &str) -> std::result::Result<(Option<u32>, Option<u32>), String> {
    let (user, group) = match spec.split_once(':') {
        Some((u, g)) => (u.trim(), Some(g.trim())),
        None => (spec.trim(), None),
    };

    let uid = if user.is_empty() {
        None
    } else {
        Some(resolve_id(user, "/etc/passwd").ok_or_else(|| format!("unknown user {user:?}"))?)
    };
    let gid = match group {
        None | Some("") => None,
        Some(g) => Some(resolve_id(g, "/etc/group").ok_or_else(|| format!("unknown group {g:?}"))?),
    };
    Ok((uid, gid))
}

/// Resolve a user/group spec to a numeric id: a numeric spec is used as-is; a name
/// is looked up in the given file-backed database (`/etc/passwd` or `/etc/group`),
/// whose `name:x:id:…` lines carry the id in the third colon-field.
#[cfg(unix)]
fn resolve_id(spec: &str, database: &str) -> Option<u32> {
    if let Ok(id) = spec.parse::<u32>() {
        return Some(id);
    }
    let contents = std::fs::read_to_string(database).ok()?;
    lookup_name_in_database(&contents, spec)
}

/// Find `name`'s numeric id in a passwd/group-format file body. The format is
/// `name:password:id:…` per line; the id is the third field. Comments/blank lines
/// are skipped.
#[cfg(unix)]
fn lookup_name_in_database(contents: &str, name: &str) -> Option<u32> {
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split(':');
        let entry_name = fields.next()?;
        if entry_name != name {
            continue;
        }
        // Skip the password field, take the id.
        let _password = fields.next();
        let id = fields.next()?;
        return id.trim().parse::<u32>().ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_mode_parses_common_forms() {
        assert_eq!(parse_octal_mode("755"), Some(0o755));
        assert_eq!(parse_octal_mode("0644"), Some(0o644));
        assert_eq!(parse_octal_mode("0o600"), Some(0o600));
        assert_eq!(parse_octal_mode("777"), Some(0o777));
    }

    #[test]
    fn octal_mode_rejects_garbage() {
        assert_eq!(parse_octal_mode(""), None);
        assert_eq!(parse_octal_mode("8"), None); // 8 is not an octal digit
        assert_eq!(parse_octal_mode("abc"), None);
        assert_eq!(parse_octal_mode("rwxr-xr-x"), None);
    }

    #[cfg(unix)]
    #[test]
    fn name_database_lookup_reads_third_field() {
        let passwd =
            "# comment\nroot:x:0:0:root:/root:/bin/sh\nmedia:x:1000:1000::/home/media:/bin/sh\n";
        assert_eq!(lookup_name_in_database(passwd, "root"), Some(0));
        assert_eq!(lookup_name_in_database(passwd, "media"), Some(1000));
        assert_eq!(lookup_name_in_database(passwd, "nobody"), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn chmod_sets_file_mode_bits() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("media.mkv");
        std::fs::write(&f, b"x").unwrap();

        let perms = ImportPermissions {
            chmod_file: Some("640".to_string()),
            ..Default::default()
        };
        let outcome = apply_to_file(&f, &perms).await;
        assert!(matches!(
            outcome,
            PermissionOutcome::Applied { chmod: true, .. }
        ));
        let mode = std::fs::metadata(&f).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn chmod_sets_folder_mode_bits() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("Series");
        std::fs::create_dir(&sub).unwrap();

        let perms = ImportPermissions {
            chmod_folder: Some("750".to_string()),
            ..Default::default()
        };
        let outcome = apply_to_folder(&sub, &perms).await;
        assert!(matches!(
            outcome,
            PermissionOutcome::Applied { chmod: true, .. }
        ));
        let mode = std::fs::metadata(&sub).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o750);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn invalid_mode_is_a_logged_failure_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("media.mkv");
        std::fs::write(&f, b"x").unwrap();
        let perms = ImportPermissions {
            chmod_file: Some("zzz".to_string()),
            ..Default::default()
        };
        let outcome = apply_to_file(&f, &perms).await;
        assert!(outcome.is_failure());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn empty_policy_applies_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("media.mkv");
        std::fs::write(&f, b"x").unwrap();
        let outcome = apply_to_file(&f, &ImportPermissions::default()).await;
        assert!(matches!(
            outcome,
            PermissionOutcome::Applied {
                chmod: false,
                chown: false,
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn chown_to_own_uid_is_noop_success() {
        // chown to our own uid (numeric) always succeeds without privilege, proving
        // the resolve + syscall path works end to end.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("media.mkv");
        std::fs::write(&f, b"x").unwrap();
        let my_uid = my_uid();
        let perms = ImportPermissions {
            chown: Some(my_uid.to_string()),
            ..Default::default()
        };
        let outcome = apply_to_file(&f, &perms).await;
        assert!(
            matches!(outcome, PermissionOutcome::Applied { chown: true, .. }),
            "{outcome:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn chown_spec_splits_user_and_group() {
        assert_eq!(resolve_chown("0:0"), Ok((Some(0), Some(0))));
        assert_eq!(resolve_chown("0"), Ok((Some(0), None)));
        assert_eq!(resolve_chown(":0"), Ok((None, Some(0))));
        assert!(resolve_chown("definitely-not-a-real-user-xyz").is_err());
    }

    /// Our own uid, read from the metadata of a file we just created (safe, no
    /// libc): a freshly created temp file is owned by the current user.
    #[cfg(unix)]
    fn my_uid() -> u32 {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("probe");
        std::fs::write(&f, b"").unwrap();
        std::fs::metadata(&f).unwrap().uid()
    }
}
