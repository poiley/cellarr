//! Live smoke-driver for the qBittorrent adapter (NOT a CI test).
//!
//! This binary exercises the real [`QbittorrentClient`] against a live
//! qBittorrent WebUI. It is intentionally an `example`, not a `#[test]`, so it
//! never runs on the offline CI path (the record/replay contract tests in
//! `tests/` are the CI coverage; see `docs/06-integrations.md`).
//!
//! It is driven by `scripts/qbittorrent-live.sh`, which stands up an ephemeral
//! Docker container, discovers the WebUI credentials within a bounded loop, and
//! tears the container down afterwards. Every external wait here is hard-bounded
//! so the driver can never wedge:
//!
//! - the `reqwest` transport applies a 10s per-call timeout (`http::DEFAULT_TIMEOUT`);
//! - the status poll loop is bounded by `CELLARR_QBIT_POLL_BUDGET_SECS` (default 30s);
//! - it NEVER waits for the torrent to finish downloading — it only waits for the
//!   torrent to *appear* under its category in any status, then deletes it.
//!
//! Configuration (all via env, supplied by the script):
//! - `CELLARR_QBIT_URL`        base WebUI URL (e.g. `http://127.0.0.1:NNNNN`)
//! - `CELLARR_QBIT_USER`       WebUI username
//! - `CELLARR_QBIT_PASS`       WebUI password
//! - `CELLARR_QBIT_MAGNET`     magnet/torrent URL to add
//! - `CELLARR_QBIT_CATEGORY`   category to file it under (e.g. `cellarr-tv`)
//! - `CELLARR_QBIT_POLL_BUDGET_SECS` optional poll budget (default 30)

use std::time::{Duration, Instant};

use cellarr_core::release::Protocol;
use cellarr_core::{
    ContentId, ContentRef, Coordinates, DownloadClientId, GrabRequest, IndexerId, LibraryId,
    MediaType, Release,
};
use cellarr_download::{DownloadError, QbittorrentClient, QbittorrentSettings};

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("missing required env var {key}"))
}

fn grab(download_url: &str, category: &str) -> GrabRequest {
    let indexer_id = IndexerId::new();
    let content_ref = ContentRef::new(
        ContentId::new(),
        LibraryId::new(),
        MediaType::Tv,
        Coordinates::Episode {
            season: 1,
            episode: 1,
            absolute: None,
        },
    )
    .expect("valid coords");
    GrabRequest {
        content_ref,
        release: Release {
            indexer_id,
            title: "Live.Smoke.Release.S01E01".into(),
            download_url: download_url.into(),
            guid: None,
            protocol: Protocol::Torrent,
            size: None,
            seeders: None,
            indexer_flags: vec![],
        },
        indexer_id,
        client_id: DownloadClientId::new(),
        category: category.into(),
        release_type: None,
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let base_url = env("CELLARR_QBIT_URL");
    let user = env("CELLARR_QBIT_USER");
    let pass = env("CELLARR_QBIT_PASS");
    let magnet = env("CELLARR_QBIT_MAGNET");
    let category = env("CELLARR_QBIT_CATEGORY");
    let poll_budget: u64 = std::env::var("CELLARR_QBIT_POLL_BUDGET_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    let settings = QbittorrentSettings {
        base_url: base_url.clone(),
        username: user.clone(),
        password: pass.clone(),
    };
    let client = QbittorrentClient::new("qbit-live", settings, category.clone());

    // 1) Version probe (also proves authenticated session over the LAN/container
    //    path, not the loopback auth-bypass).
    let version = client.version().await.expect("version probe failed");
    println!("LIVE qbittorrent_version={version}");

    // 2) Auth-failure path: a deliberately wrong password must yield a typed Auth
    //    error, never a generic failure.
    {
        let bad = QbittorrentClient::new(
            "qbit-bad",
            QbittorrentSettings {
                base_url: base_url.clone(),
                username: user.clone(),
                password: format!("{pass}-WRONG"),
            },
            category.clone(),
        );
        match bad.version().await {
            Err(DownloadError::Auth(msg)) => println!("LIVE auth_failure_ok=\"{msg}\""),
            other => panic!("expected DownloadError::Auth on wrong password, got {other:?}"),
        }
    }

    // 3) Add under the cellarr category.
    let g = grab(&magnet, &category);
    let id = client.add(&g).await.expect("add failed");
    println!("LIVE added_hash={id}");

    // 4) Poll ONCE-with-retry (bounded) until the torrent APPEARS under its
    //    category in ANY status. NEVER wait for completion.
    let deadline = Instant::now() + Duration::from_secs(poll_budget);
    let mut appeared = false;
    let mut last_state = String::from("<none>");
    while Instant::now() < deadline {
        match client.progress(&id).await {
            Ok(p) => {
                last_state = format!("{:?}", p.state);
                if p.is_in_category(&category) {
                    appeared = true;
                    println!(
                        "LIVE tracked_state={:?} category_ok=true content_path={:?}",
                        p.state, p.content_path
                    );
                    break;
                }
                // Present but category not yet reflected: re-file explicitly and
                // retry (exercises setCategory on the live client too).
                let _ = client.set_category(&id, &category).await;
            }
            // Not visible yet (metadata still resolving): bounded retry.
            Err(DownloadError::NotFound(_)) => {}
            Err(e) => panic!("unexpected error polling status: {e:?}"),
        }
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }

    if !appeared {
        // Clean up before failing so the container teardown isn't the only path
        // that removes the torrent.
        let _ = client
            .remove(&id, cellarr_download::RemovePolicy::immediate(true))
            .await;
        panic!("torrent {id} did not appear under category {category} within {poll_budget}s (last_state={last_state})");
    }

    // 5) Remove it (delete data) — unconditional immediate removal.
    let removed = client
        .remove(&id, cellarr_download::RemovePolicy::immediate(true))
        .await
        .expect("remove failed");
    assert!(removed, "remove returned false under an immediate policy");
    println!("LIVE removed_hash={id} ok=true");

    println!("LIVE_RESULT=PASS version={version}");
}
