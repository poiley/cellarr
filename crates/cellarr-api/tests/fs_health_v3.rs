//! The cross-filesystem (silent-copy-fallback) health warning, end-to-end.
//!
//! Proves the deliberate differentiator is genuinely wired: a configured
//! downloads dir and a library root on DIFFERENT filesystems raise a loud
//! warning surfaced on `/api/v3/health` (both faces), and a same-filesystem
//! layout raises nothing. The cross-fs case needs a real second filesystem,
//! which we provision with a RAM disk on macOS and self-skip cleanly when it is
//! unavailable (so the suite never wedges or fails on a host without it).

mod common;

use common::start_open;
use serde_json::Value;

use cellarr_core::{
    DownloadClientConfig, DownloadClientId, Library, LibraryId, MediaType, Protocol,
};

/// Seed a library whose single root is `root`, and a download client whose
/// configured downloads dir is `downloads`.
async fn seed_layout(state: &cellarr_api::AppState, root: &str, downloads: &str) {
    let profile_id = common::seed_profile(state, "fs-health-profile").await;
    let library = Library {
        id: LibraryId::new(),
        media_type: MediaType::Movie,
        name: "fs-health-lib".to_string(),
        root_folders: vec![root.to_string()],
        default_quality_profile: profile_id,
    };
    state
        .db
        .config()
        .upsert_library(&library)
        .await
        .expect("seed library");

    let client = DownloadClientConfig {
        tags: Vec::new(),
        id: DownloadClientId::new(),
        name: "fs-health-client".to_string(),
        kind: "qbittorrent".to_string(),
        protocol: Protocol::Torrent,
        enabled: true,
        priority: 1,
        category: "cellarr".to_string(),
        settings: serde_json::json!({ "download_dir": downloads }),
    };
    state
        .db
        .config()
        .upsert_download_client(&client)
        .await
        .expect("seed download client");
}

/// Fetch `/api/v3/health` and return the parsed array.
async fn health(server: &common::TestServer) -> Vec<Value> {
    let resp = server
        .client()
        .get(server.url("/api/v3/health"))
        .send()
        .await
        .expect("health request");
    assert_eq!(resp.status(), 200);
    resp.json::<Vec<Value>>().await.expect("health json")
}

fn has_cross_fs_warning(records: &[Value]) -> bool {
    records.iter().any(|r| {
        r.get("source").and_then(Value::as_str) == Some("ImportMechanismCheck")
            && r.get("type").and_then(Value::as_str) == Some("warning")
            && r.get("message")
                .and_then(Value::as_str)
                .map(|m| m.contains("DIFFERENT filesystem"))
                .unwrap_or(false)
    })
}

#[tokio::test]
async fn same_filesystem_layout_raises_no_cross_fs_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let downloads = tmp.path().join("downloads");
    let library = tmp.path().join("library/movies");
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::create_dir_all(&library).unwrap();

    let server = start_open().await;
    seed_layout(
        &server.state,
        library.to_str().unwrap(),
        downloads.to_str().unwrap(),
    )
    .await;

    let records = health(&server).await;
    assert!(
        !has_cross_fs_warning(&records),
        "same-fs layout must not warn; got {records:?}"
    );
}

/// A RAM disk path (macOS) for a genuine second filesystem, or `None` if one
/// cannot be created on this host. Returns the mount path and the BSD device so
/// the caller can detach it.
#[cfg(target_os = "macos")]
fn make_ramdisk() -> Option<(std::path::PathBuf, String)> {
    use std::process::Command;
    // 32 MiB ram disk: `hdiutil attach -nomount ram://65536` → /dev/diskN
    let attach = Command::new("hdiutil")
        .args(["attach", "-nomount", "ram://65536"])
        .output()
        .ok()?;
    if !attach.status.success() {
        return None;
    }
    let dev = String::from_utf8_lossy(&attach.stdout).trim().to_string();
    if dev.is_empty() {
        return None;
    }
    let mount = std::env::temp_dir().join(format!("cellarr-ramdisk-{}", std::process::id()));
    std::fs::create_dir_all(&mount).ok()?;
    let fmt = Command::new("newfs_hfs").arg(&dev).output().ok();
    let ok = fmt.map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        let _ = Command::new("hdiutil").args(["detach", &dev]).output();
        return None;
    }
    let m = Command::new("mount")
        .args(["-t", "hfs", &dev, mount.to_str()?])
        .output()
        .ok()?;
    if !m.status.success() {
        let _ = Command::new("hdiutil")
            .args(["detach", "-force", &dev])
            .output();
        return None;
    }
    Some((mount, dev))
}

/// Tear a RAM disk down robustly: a mount made via `mount -t hfs` must be
/// `umount`ed before `hdiutil detach` will release the device (otherwise it is
/// "Resource busy"). We umount the mount point, then force-detach the device,
/// then remove the now-empty mount dir — so the test never leaks a RAM disk.
#[cfg(target_os = "macos")]
fn detach_ramdisk(mount: &std::path::Path, dev: &str) {
    use std::process::Command;
    let _ = Command::new("umount").arg(mount).output();
    let _ = Command::new("hdiutil")
        .args(["detach", "-force", dev])
        .output();
    let _ = std::fs::remove_dir_all(mount);
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn cross_filesystem_layout_raises_a_loud_health_warning() {
    let Some((ramdisk, dev)) = make_ramdisk() else {
        eprintln!("SKIP: could not create a RAM disk (no second filesystem available)");
        return;
    };

    // Library lives on a normal tempdir; downloads live on the RAM disk → two
    // genuinely different filesystems (distinct st_dev).
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library/movies");
    std::fs::create_dir_all(&library).unwrap();
    let downloads = ramdisk.join("downloads");
    std::fs::create_dir_all(&downloads).unwrap();

    let server = start_open().await;
    seed_layout(
        &server.state,
        library.to_str().unwrap(),
        downloads.to_str().unwrap(),
    )
    .await;

    let records = health(&server).await;
    let warned = has_cross_fs_warning(&records);

    // Always detach the RAM disk before asserting so a failure never leaks it.
    detach_ramdisk(&ramdisk, &dev);

    assert!(
        warned,
        "cross-fs layout MUST raise the loud ImportMechanismCheck warning; got {records:?}"
    );
}
