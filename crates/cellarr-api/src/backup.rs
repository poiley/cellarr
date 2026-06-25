//! Database backup & restore — consistent snapshots bundled with config.
//!
//! Mirrors the Sonarr/Radarr `system/backup` surface: a backup is a single,
//! self-contained, downloadable file holding a **consistent snapshot of the
//! SQLite database** (taken via [`Database::snapshot_to`], i.e. `VACUUM INTO`)
//! plus the daemon's effective config, under a timestamped name in a `backups`
//! directory. The engine lists, creates, serves, deletes, and restores them, and
//! prunes old backups to a retention bound.
//!
//! ## Why a custom single-file bundle
//! The originals ship a `.zip`. cellarr deliberately avoids pulling an archive
//! crate (single-binary/lean-default non-negotiable) and instead uses a tiny,
//! fully-controlled container format ([`MAGIC`] header + length-prefixed manifest
//! and DB sections). It is a single file the UI can download and re-upload, and
//! restore reads it back deterministically. The format is versioned so a future
//! change can migrate.
//!
//! ## Restore safety (the destructive path)
//! Restore **replaces the live database**, so it is fenced:
//! 1. take an automatic **pre-restore safety backup** of the current DB first, so
//!    a bad restore is itself reversible;
//! 2. **validate** the bundle and the DB snapshot it carries
//!    ([`Database::verify_snapshot`] — `PRAGMA integrity_check`) *before* touching
//!    the live file;
//! 3. write the snapshot to a temp file beside the live DB and **atomically
//!    rename** it into place (same-directory `rename` is atomic on the platforms
//!    we target), removing the stale WAL/`-shm` sidecars so the swapped-in file is
//!    authoritative.
//!
//! A running daemon holds the old DB's pool open, so the swapped-in file takes
//! full effect on the **next start**; the response says so. The DB is never left
//! half-written: either the atomic rename succeeded (new DB live) or it did not
//! (old DB untouched), and the pre-restore safety copy is always present.
//!
//! Postgres backends are not handled here yet — see [`BackupEngine::create`].

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use cellarr_db::Database;
use serde::{Deserialize, Serialize};

/// The bundle magic + format version. Bumped if the container layout changes.
const MAGIC: &[u8] = b"CELLARRBKP1\n";

/// Errors from the backup/restore engine.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    /// An I/O error touching the backups directory or a bundle file.
    #[error("backup io error: {0}")]
    Io(String),
    /// The persistence layer failed to produce or validate a snapshot.
    #[error("backup db error: {0}")]
    Db(#[from] cellarr_db::DbError),
    /// A bundle file was malformed (bad magic, truncated, or unreadable manifest).
    #[error("malformed backup bundle: {0}")]
    Malformed(String),
    /// The addressed backup id does not exist.
    #[error("backup {0} not found")]
    NotFound(String),
    /// Restore is unsupported for the active backend (Postgres — deferred).
    #[error("{0}")]
    Unsupported(String),
}

type Result<T> = std::result::Result<T, BackupError>;

/// The manifest stored at the head of a bundle: metadata plus the daemon's
/// effective config at backup time, so a restore re-establishes the same wiring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    /// cellarr version that produced the bundle.
    pub app_version: String,
    /// When the backup was taken (unix seconds).
    pub created_unix: i64,
    /// `"manual"` (operator-triggered) or `"scheduled"` (the recurring job).
    pub kind: String,
    /// The effective config JSON captured at backup time (advisory — restore
    /// swaps the DB, not the running config). `null` when no config was supplied.
    #[serde(default)]
    pub config: serde_json::Value,
    /// Length in bytes of the DB snapshot section that follows the manifest.
    pub db_len: u64,
}

/// A backup as listed on the API surface.
#[derive(Debug, Clone, Serialize)]
pub struct BackupInfo {
    /// The stable id (the bundle file name without extension).
    pub id: String,
    /// The file name on disk.
    pub name: String,
    /// Size of the bundle file in bytes.
    pub size: u64,
    /// When it was taken (unix seconds), read from the manifest.
    pub created_unix: i64,
    /// `"manual"` or `"scheduled"`.
    pub kind: String,
}

/// The backup engine, bound to a backups directory and the live database.
#[derive(Clone)]
pub struct BackupEngine {
    dir: PathBuf,
    db: Database,
    /// The live SQLite database file path, needed for the atomic restore swap.
    db_path: PathBuf,
}

/// The bundle file extension.
const EXT: &str = "cbk";

impl BackupEngine {
    /// Bind the engine to its backups `dir` and the live `db` (whose file lives at
    /// `db_path`). The directory is created on first [`create`](Self::create).
    #[must_use]
    pub fn new(dir: PathBuf, db: Database, db_path: PathBuf) -> Self {
        Self { dir, db, db_path }
    }

    /// The backups directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The absolute path of a backup id's bundle file, with traversal rejected:
    /// the id must be a bare file stem (no separators, no `..`), so a crafted id
    /// can never address a file outside the backups directory.
    fn path_for(&self, id: &str) -> Result<PathBuf> {
        if id.is_empty()
            || id.contains('/')
            || id.contains('\\')
            || id.contains("..")
            || id.contains('\0')
        {
            return Err(BackupError::NotFound(id.to_string()));
        }
        Ok(self.dir.join(format!("{id}.{EXT}")))
    }

    /// Create a new backup bundle: a consistent DB snapshot + the supplied config
    /// JSON, written atomically under a timestamped name. Returns its listing.
    ///
    /// `kind` is `"manual"` or `"scheduled"`. `config` is captured into the
    /// manifest (pass `Value::Null` if unavailable).
    ///
    /// # Postgres (DEFERRED)
    /// This path is SQLite-only: it snapshots the SQLite file via `VACUUM INTO`.
    /// A Postgres backend would need a `pg_dump`-style logical dump instead.
    // TODO(postgres-backup): when the Postgres backend lands, branch on the active
    // backend and produce a logical dump here; the bundle container and restore
    // safety flow (pre-restore safety backup → validate → atomic swap) are reused.
    ///
    /// # Errors
    /// Returns a [`BackupError`] if the directory cannot be created, the snapshot
    /// fails, or the bundle cannot be written.
    pub async fn create(&self, kind: &str, config: serde_json::Value) -> Result<BackupInfo> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| BackupError::Io(format!("creating backups dir: {e}")))?;

        // Derive a unique id; on the rare same-nanosecond collision (two backups
        // in a very tight loop) nudge the suffix so we never overwrite a sibling.
        let now = time::OffsetDateTime::now_utc();
        let mut id = backup_id(now, kind);
        let mut bundle_path = self.path_for(&id)?;
        let mut bump = 0u32;
        while bundle_path.exists() {
            bump += 1;
            id = format!("{}_{bump}", backup_id(now, kind));
            bundle_path = self.path_for(&id)?;
        }

        // Snapshot into a temp file beside the bundle (same dir → cheap to read
        // back and bundle, and a partial bundle never appears under the final id).
        let tmp_snap = self.dir.join(format!(".{id}.snap.tmp"));
        let _ = std::fs::remove_file(&tmp_snap); // clear any stale temp
        self.db.snapshot_to(&tmp_snap).await?;

        let snap_bytes = std::fs::read(&tmp_snap)
            .map_err(|e| BackupError::Io(format!("reading snapshot: {e}")))?;
        let _ = std::fs::remove_file(&tmp_snap);

        let manifest = BackupManifest {
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            created_unix: now.unix_timestamp(),
            kind: kind.to_string(),
            config,
            db_len: snap_bytes.len() as u64,
        };

        // Write the bundle to a temp file, then atomically rename into place so a
        // crash mid-write never leaves a half-bundle under the real name.
        let tmp_bundle = self.dir.join(format!(".{id}.bundle.tmp"));
        write_bundle(&tmp_bundle, &manifest, &snap_bytes)?;
        std::fs::rename(&tmp_bundle, &bundle_path)
            .map_err(|e| BackupError::Io(format!("finalizing bundle: {e}")))?;

        self.info_for(&id)
    }

    /// List all backups, newest first.
    ///
    /// # Errors
    /// Returns a [`BackupError`] if the directory cannot be read. A bundle file
    /// whose manifest is unreadable is skipped (not fatal to the listing).
    pub fn list(&self) -> Result<Vec<BackupInfo>> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            // No directory yet → no backups (not an error).
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(BackupError::Io(format!("reading backups dir: {e}"))),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(EXT) {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if let Ok(info) = self.info_for(id) {
                out.push(info);
            }
        }
        out.sort_by(|a, b| b.created_unix.cmp(&a.created_unix).then(b.id.cmp(&a.id)));
        Ok(out)
    }

    /// Read one backup's listing by id.
    ///
    /// # Errors
    /// [`BackupError::NotFound`] if absent, [`BackupError::Malformed`] if the
    /// bundle header/manifest cannot be read.
    pub fn info_for(&self, id: &str) -> Result<BackupInfo> {
        let path = self.path_for(id)?;
        let meta = std::fs::metadata(&path).map_err(|_| BackupError::NotFound(id.to_string()))?;
        let manifest = read_manifest(&path)?;
        Ok(BackupInfo {
            id: id.to_string(),
            name: format!("{id}.{EXT}"),
            size: meta.len(),
            created_unix: manifest.created_unix,
            kind: manifest.kind,
        })
    }

    /// The raw bundle bytes for download.
    ///
    /// # Errors
    /// [`BackupError::NotFound`] if the id is absent or invalid.
    pub fn read_bundle(&self, id: &str) -> Result<Vec<u8>> {
        let path = self.path_for(id)?;
        std::fs::read(&path).map_err(|_| BackupError::NotFound(id.to_string()))
    }

    /// Delete a backup by id. Idempotent: deleting an absent id succeeds.
    ///
    /// # Errors
    /// Returns a [`BackupError`] only on an unexpected I/O failure (not for an
    /// already-missing file).
    pub fn delete(&self, id: &str) -> Result<()> {
        let path = self.path_for(id)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(BackupError::Io(format!("deleting backup {id}: {e}"))),
        }
    }

    /// Prune backups so that at most `keep` remain (the newest `keep`), deleting
    /// the rest. A `keep` of `0` is treated as "keep at least 1" so retention can
    /// never wipe every backup. Returns the ids pruned.
    ///
    /// # Errors
    /// Returns a [`BackupError`] if the directory cannot be listed.
    pub fn prune(&self, keep: usize) -> Result<Vec<String>> {
        let keep = keep.max(1);
        let all = self.list()?;
        let mut pruned = Vec::new();
        for info in all.into_iter().skip(keep) {
            self.delete(&info.id)?;
            pruned.push(info.id);
        }
        Ok(pruned)
    }

    /// Restore the live database from a backup **id** already in the backups dir.
    /// See [`Self::restore_from_bytes`] for the safety flow.
    ///
    /// # Errors
    /// [`BackupError::NotFound`] if the id is absent; otherwise as
    /// [`Self::restore_from_bytes`].
    pub async fn restore_id(&self, id: &str) -> Result<RestoreOutcome> {
        let bytes = self.read_bundle(id)?;
        self.restore_from_bytes(&bytes).await
    }

    /// Restore the live database from an **uploaded** bundle's bytes.
    ///
    /// The destructive path, fenced for safety (see the module docs):
    /// 1. validate the bundle header + manifest, extract the DB snapshot to a temp
    ///    file, and run `PRAGMA integrity_check` on it — *before* touching the
    ///    live DB. A bad upload fails here with the live DB untouched.
    /// 2. take an automatic **pre-restore safety backup** of the current live DB.
    /// 3. atomically rename the validated snapshot over the live DB file, and
    ///    remove the stale `-wal`/`-shm` sidecars.
    ///
    /// The running pool still points at the old file's pages until the daemon
    /// restarts, so [`RestoreOutcome::restart_required`] is always `true`.
    ///
    /// # Errors
    /// [`BackupError::Malformed`] if the bundle is invalid, [`BackupError::Db`] if
    /// the snapshot fails its integrity check, [`BackupError::Io`] on a filesystem
    /// failure during the swap. The pre-restore safety backup is taken before any
    /// destructive step, so the prior state is always recoverable.
    pub async fn restore_from_bytes(&self, bundle: &[u8]) -> Result<RestoreOutcome> {
        // Postgres restore is not modeled (the swap is a SQLite-file rename).
        // TODO(postgres-restore): restore a logical dump for the Postgres backend.
        if self.db_path.as_os_str().is_empty() {
            return Err(BackupError::Unsupported(
                "restore is only supported for the SQLite backend".into(),
            ));
        }

        // 1. Parse + validate the bundle and extract the snapshot to a temp file.
        let (_manifest, snap_bytes) = parse_bundle(bundle)?;
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| BackupError::Io(format!("creating backups dir: {e}")))?;
        let staged = self.dir.join(format!(
            ".restore.{}.staged",
            time::OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        std::fs::write(&staged, &snap_bytes)
            .map_err(|e| BackupError::Io(format!("staging restore snapshot: {e}")))?;
        // Validate BEFORE touching the live DB; a corrupt upload stops here.
        if let Err(e) = Database::verify_snapshot(
            staged
                .to_str()
                .ok_or_else(|| BackupError::Io("staged path not UTF-8".into()))?,
        )
        .await
        {
            let _ = std::fs::remove_file(&staged);
            return Err(BackupError::Db(e));
        }

        // 2. Pre-restore safety backup of the CURRENT live DB (reversible restore).
        let safety = self.create("pre-restore", serde_json::Value::Null).await?;

        // 3. Atomic swap: rename the validated snapshot over the live DB, then drop
        //    the stale WAL/-shm sidecars so the swapped-in file is authoritative.
        if let Err(e) = std::fs::rename(&staged, &self.db_path) {
            let _ = std::fs::remove_file(&staged);
            return Err(BackupError::Io(format!(
                "atomic swap onto live database failed (live DB untouched; \
                 pre-restore safety backup {} present): {e}",
                safety.id
            )));
        }
        for sidecar in ["-wal", "-shm"] {
            let mut p = self.db_path.clone().into_os_string();
            p.push(sidecar);
            let _ = std::fs::remove_file(PathBuf::from(p));
        }

        Ok(RestoreOutcome {
            safety_backup_id: safety.id,
            restart_required: true,
        })
    }
}

/// The result of a restore: which pre-restore safety backup was taken, and that a
/// daemon restart is needed for the swapped-in DB to take full effect.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreOutcome {
    /// The id of the automatic pre-restore safety backup of the prior DB.
    pub safety_backup_id: String,
    /// Always `true`: the live pool still holds the old file until restart.
    pub restart_required: bool,
}

/// Build a sortable, unique backup id from a timestamp + kind, e.g.
/// `cellarr_backup_manual_20260624T120000Z` — lexical order matches chronological
/// order, and the kind is embedded for at-a-glance listing.
fn backup_id(now: time::OffsetDateTime, kind: &str) -> String {
    use time::format_description::FormatItem;
    use time::macros::format_description;
    const FMT: &[FormatItem<'static>] =
        format_description!("[year][month][day]T[hour][minute][second]");
    // Sub-second nanosecond suffix (9 digits) guards against two backups in the
    // same second (e.g. a pre-restore backup immediately followed by a scheduled
    // one) and keeps lexical id order == chronological order: the second-granular
    // stamp orders across seconds, the sub-second suffix orders within one.
    let stamp = now.format(FMT).unwrap_or_else(|_| "00000000T000000".into());
    let subsec_nanos = now.nanosecond();
    let kind = sanitize_kind(kind);
    format!("cellarr_backup_{kind}_{stamp}Z_{subsec_nanos:09}")
}

/// Keep a `kind` label filename-safe (alphanumerics + dash), so it can never
/// inject a path separator into an id.
fn sanitize_kind(kind: &str) -> String {
    let s: String = kind
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if s.is_empty() {
        "manual".into()
    } else {
        s
    }
}

/// Write a bundle file: magic, length-prefixed manifest JSON, then the DB bytes.
fn write_bundle(path: &Path, manifest: &BackupManifest, db_bytes: &[u8]) -> Result<()> {
    let manifest_json = serde_json::to_vec(manifest)
        .map_err(|e| BackupError::Malformed(format!("encoding manifest: {e}")))?;
    let file = std::fs::File::create(path)
        .map_err(|e| BackupError::Io(format!("creating bundle: {e}")))?;
    let mut w = std::io::BufWriter::new(file);
    let io = |e: std::io::Error| BackupError::Io(format!("writing bundle: {e}"));
    w.write_all(MAGIC).map_err(io)?;
    w.write_all(&(manifest_json.len() as u64).to_le_bytes())
        .map_err(io)?;
    w.write_all(&manifest_json).map_err(io)?;
    w.write_all(&(db_bytes.len() as u64).to_le_bytes())
        .map_err(io)?;
    w.write_all(db_bytes).map_err(io)?;
    w.flush().map_err(io)?;
    Ok(())
}

/// Read just the manifest from a bundle file (cheap — does not load the DB bytes).
fn read_manifest(path: &Path) -> Result<BackupManifest> {
    let mut f =
        std::fs::File::open(path).map_err(|e| BackupError::Io(format!("opening bundle: {e}")))?;
    let mut magic = [0u8; MAGIC.len()];
    f.read_exact(&mut magic)
        .map_err(|e| BackupError::Malformed(format!("reading magic: {e}")))?;
    if magic != MAGIC {
        return Err(BackupError::Malformed("bad magic / wrong format".into()));
    }
    let mut len_buf = [0u8; 8];
    f.read_exact(&mut len_buf)
        .map_err(|e| BackupError::Malformed(format!("reading manifest length: {e}")))?;
    let mlen = u64::from_le_bytes(len_buf) as usize;
    // Guard against an absurd manifest length pointing at a corrupt/hostile file.
    if mlen > 1 << 20 {
        return Err(BackupError::Malformed("manifest length implausible".into()));
    }
    let mut mbuf = vec![0u8; mlen];
    f.read_exact(&mut mbuf)
        .map_err(|e| BackupError::Malformed(format!("reading manifest: {e}")))?;
    serde_json::from_slice(&mbuf)
        .map_err(|e| BackupError::Malformed(format!("decoding manifest: {e}")))
}

/// Parse a full in-memory bundle into its manifest and DB snapshot bytes.
fn parse_bundle(bundle: &[u8]) -> Result<(BackupManifest, Vec<u8>)> {
    let mut cur = bundle;
    let take = |cur: &mut &[u8], n: usize, what: &str| -> Result<Vec<u8>> {
        if cur.len() < n {
            return Err(BackupError::Malformed(format!("truncated {what}")));
        }
        let (head, tail) = cur.split_at(n);
        *cur = tail;
        Ok(head.to_vec())
    };
    let magic = take(&mut cur, MAGIC.len(), "magic")?;
    if magic != MAGIC {
        return Err(BackupError::Malformed("bad magic / wrong format".into()));
    }
    let mlen = u64::from_le_bytes(
        take(&mut cur, 8, "manifest length")?
            .try_into()
            .map_err(|_| BackupError::Malformed("manifest length".into()))?,
    ) as usize;
    if mlen > 1 << 20 {
        return Err(BackupError::Malformed("manifest length implausible".into()));
    }
    let manifest_bytes = take(&mut cur, mlen, "manifest")?;
    let manifest: BackupManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| BackupError::Malformed(format!("decoding manifest: {e}")))?;
    let dlen = u64::from_le_bytes(
        take(&mut cur, 8, "db length")?
            .try_into()
            .map_err(|_| BackupError::Malformed("db length".into()))?,
    ) as usize;
    let db_bytes = take(&mut cur, dlen, "db snapshot")?;
    Ok((manifest, db_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn temp_db(dir: &Path) -> (Database, PathBuf) {
        let path = dir.join("cellarr.sqlite");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        (db, path)
    }

    #[test]
    fn rejects_path_traversal_ids() {
        let dir = tempfile::tempdir().unwrap();
        // db not needed for the pure path check; build with an in-memory stand-in
        // is awkward, so just construct the engine with a dummy db handle below.
        // (covered fully in the async tests; this asserts the id guard directly.)
        let _ = dir;
        for bad in ["..", "../escape", "a/b", "a\\b", "", "x/../y"] {
            // Build a minimal engine purely to exercise path_for's guard.
            let eng = BackupEngine {
                dir: PathBuf::from("/tmp/none"),
                db: dummy_db(),
                db_path: PathBuf::from("/tmp/none/db.sqlite"),
            };
            assert!(eng.path_for(bad).is_err(), "should reject id {bad:?}");
        }
    }

    // A throwaway Database for the synchronous guard test. We never touch it.
    fn dummy_db() -> Database {
        // Build one on a blocking thread since Database::open is async; the guard
        // test does not use it, but the struct needs a value.
        std::thread::scope(|s| {
            s.spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async { Database::open_in_memory().await.unwrap() })
            })
            .join()
            .unwrap()
        })
    }

    #[tokio::test]
    async fn backup_round_trips_and_lists() {
        let dir = tempfile::tempdir().unwrap();
        let (db, db_path) = temp_db(dir.path()).await;
        let eng = BackupEngine::new(dir.path().join("backups"), db.clone(), db_path);

        let info = eng
            .create("manual", serde_json::json!({ "k": "v" }))
            .await
            .unwrap();
        assert_eq!(info.kind, "manual");
        assert!(info.size > 0);

        let list = eng.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, info.id);

        // The downloadable bytes parse back to the same manifest + a valid DB.
        let bytes = eng.read_bundle(&info.id).unwrap();
        let (manifest, db_bytes) = parse_bundle(&bytes).unwrap();
        assert_eq!(manifest.config, serde_json::json!({ "k": "v" }));
        assert_eq!(manifest.db_len as usize, db_bytes.len());

        db.shutdown().await;
    }

    #[tokio::test]
    async fn restore_round_trips_to_original_state_with_safety_backup() {
        let dir = tempfile::tempdir().unwrap();
        let (db, db_path) = temp_db(dir.path()).await;
        let eng = BackupEngine::new(dir.path().join("backups"), db.clone(), db_path.clone());

        // Original state: one root folder.
        let folder = cellarr_core::RootFolder {
            id: uuid::Uuid::new_v4().to_string(),
            path: "/original".into(),
            name: None,
            enabled: true,
        };
        db.config().upsert_root_folder(&folder).await.unwrap();

        // Backup, then MUTATE: add a second folder the backup doesn't have.
        let backup = eng.create("manual", serde_json::Value::Null).await.unwrap();
        let folder2 = cellarr_core::RootFolder {
            id: uuid::Uuid::new_v4().to_string(),
            path: "/added-after-backup".into(),
            name: None,
            enabled: true,
        };
        db.config().upsert_root_folder(&folder2).await.unwrap();
        assert_eq!(db.config().list_root_folders().await.unwrap().len(), 2);

        // Restore runs against the LIVE daemon: the pre-restore safety backup is
        // taken from the still-open pool, then the validated snapshot is renamed
        // over the live DB file (the live pool keeps the old inode until restart —
        // hence restart_required).
        let outcome = eng.restore_id(&backup.id).await.unwrap();
        assert!(outcome.restart_required);

        // Now the daemon "restarts": drop the old pool and reopen the swapped file.
        db.shutdown().await;

        // The pre-restore safety backup of the 2-folder state must be present.
        assert!(eng
            .list()
            .unwrap()
            .iter()
            .any(|b| b.id == outcome.safety_backup_id));

        // Reopen: the restored DB is back to the ORIGINAL single-folder state.
        let reopened = Database::open(db_path.to_str().unwrap()).await.unwrap();
        let folders = reopened.config().list_root_folders().await.unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].path, "/original");
        reopened.shutdown().await;
    }

    #[tokio::test]
    async fn retention_prunes_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let (db, db_path) = temp_db(dir.path()).await;
        let eng = BackupEngine::new(dir.path().join("backups"), db.clone(), db_path);

        // Three backups; the nanosecond suffix keeps ids distinct + ordered.
        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(
                eng.create("scheduled", serde_json::Value::Null)
                    .await
                    .unwrap()
                    .id,
            );
        }
        assert_eq!(eng.list().unwrap().len(), 3);

        let pruned = eng.prune(2).unwrap();
        assert_eq!(pruned.len(), 1);
        let remaining = eng.list().unwrap();
        assert_eq!(remaining.len(), 2);
        // The pruned one is the oldest (first created).
        assert_eq!(pruned[0], ids[0]);

        // keep=0 is clamped to keep at least 1 — retention never wipes everything.
        let pruned2 = eng.prune(0).unwrap();
        assert_eq!(pruned2.len(), 1);
        assert_eq!(eng.list().unwrap().len(), 1);

        db.shutdown().await;
    }

    #[tokio::test]
    async fn restore_rejects_corrupt_bundle_without_touching_live_db() {
        let dir = tempfile::tempdir().unwrap();
        let (db, db_path) = temp_db(dir.path()).await;
        let eng = BackupEngine::new(dir.path().join("backups"), db.clone(), db_path.clone());

        // A valid-magic bundle whose DB section is garbage.
        let manifest = BackupManifest {
            app_version: "test".into(),
            created_unix: 0,
            kind: "manual".into(),
            config: serde_json::Value::Null,
            db_len: 4,
        };
        let mut bundle = Vec::new();
        let mjson = serde_json::to_vec(&manifest).unwrap();
        bundle.extend_from_slice(MAGIC);
        bundle.extend_from_slice(&(mjson.len() as u64).to_le_bytes());
        bundle.extend_from_slice(&mjson);
        bundle.extend_from_slice(&4u64.to_le_bytes());
        bundle.extend_from_slice(b"junk");

        let err = eng.restore_from_bytes(&bundle).await.unwrap_err();
        assert!(matches!(err, BackupError::Db(_)));

        // Live DB still good — no pre-restore backup was even taken.
        db.config().list_root_folders().await.unwrap();
        assert!(eng.list().unwrap().is_empty());
        db.shutdown().await;
    }
}
