//! Live smoke-driver for the Transmission adapter (NOT a CI test).
//!
//! Exercises the real [`TransmissionClient`] against a live Transmission RPC
//! daemon. It is an `example`, not a `#[test]`, so it never runs on the offline
//! CI path (the record/replay contract tests in `tests/` are the CI coverage).
//!
//! Every external wait is hard-bounded: the `reqwest` transport applies a 10s
//! per-call timeout, and the appear-poll loop is bounded by
//! `CELLARR_TR_POLL_BUDGET_SECS` (default 30s). The torrent is added **paused**
//! and removed with `delete-local-data=true`, so nothing is ever downloaded.
//!
//! Configuration (all via env):
//! - `CELLARR_TR_URL`        base RPC URL (e.g. `http://127.0.0.1:19091`)
//! - `CELLARR_TR_USER`       optional RPC username (HTTP Basic)
//! - `CELLARR_TR_PASS`       optional RPC password
//! - `CELLARR_TR_MAGNET`     magnet/torrent URL to add
//! - `CELLARR_TR_CATEGORY`   label to file it under (e.g. `cellarr-e2e`)
//! - `CELLARR_TR_POLL_BUDGET_SECS` optional poll budget (default 30)

use std::time::{Duration, Instant};

use cellarr_core::release::Protocol;
use cellarr_core::{
    ContentId, ContentRef, Coordinates, DownloadClientId, DownloadState, GrabRequest, IndexerId,
    LibraryId, MediaType, Release,
};
use cellarr_download::{RemovePolicy, TransmissionClient, TransmissionSettings};

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("missing required env var {key}"))
}

fn opt_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
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
    let base_url = env("CELLARR_TR_URL");
    let magnet = env("CELLARR_TR_MAGNET");
    let category = env("CELLARR_TR_CATEGORY");
    let poll_budget: u64 = std::env::var("CELLARR_TR_POLL_BUDGET_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    let settings = TransmissionSettings {
        base_url: Some(base_url.clone()),
        host: None,
        port: None,
        url_base: None,
        // Throwaway download-dir: the torrent is added PAUSED and removed with
        // delete-local-data, so nothing is ever written here.
        download_dir: opt_env("CELLARR_TR_DOWNLOAD_DIR"),
        username: opt_env("CELLARR_TR_USER"),
        password: opt_env("CELLARR_TR_PASS"),
    };
    let client = TransmissionClient::new("transmission-live", settings, category.clone());

    // 1) Connect: a status call against a non-existent hash completes the CSRF
    //    409 session-id handshake and proves the RPC is reachable. A NotFound is
    //    the expected, successful outcome (the daemon answered "success" with an
    //    empty torrents array).
    match client.status("0000000000000000000000000000000000000000").await {
        Err(cellarr_download::DownloadError::NotFound(_)) => {
            println!("LIVE handshake_ok=true (session-id captured, empty torrent-get)");
        }
        Ok(state) => println!("LIVE handshake_ok=true probe_state={state:?}"),
        Err(e) => panic!("handshake/connect failed: {e:?}"),
    }

    // 2) Add the magnet PAUSED under the cellarr-e2e label.
    let g = grab(&magnet, &category);
    let id = client.add(&g, true).await.expect("paused add failed");
    println!("LIVE added_hash={id}");

    // 3) Poll (bounded) until the torrent appears under its label, and assert it
    //    is in a paused/stopped (not actively downloading/completed) state.
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
                        "LIVE tracked_state={:?} category_ok=true progress={} content_path={:?}",
                        p.state, p.progress, p.content_path
                    );
                    // A paused add must not be actively downloading.
                    assert!(
                        matches!(p.state, DownloadState::Queued | DownloadState::Completed),
                        "paused add should be queued/stopped, got {:?}",
                        p.state
                    );
                    break;
                }
            }
            Err(cellarr_download::DownloadError::NotFound(_)) => {}
            Err(e) => panic!("unexpected error polling status: {e:?}"),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    if !appeared {
        let _ = client.remove(&id, RemovePolicy::immediate(true)).await;
        panic!(
            "torrent {id} did not appear under label {category} within {poll_budget}s (last_state={last_state})"
        );
    }

    // 4) Remove it (delete local data) so NOTHING is left on the daemon/NAS.
    let removed = client
        .remove(&id, RemovePolicy::immediate(true))
        .await
        .expect("remove failed");
    assert!(removed, "remove returned false under an immediate policy");
    println!("LIVE removed_hash={id} delete_local_data=true ok=true");

    // 5) Confirm it is gone.
    match client.status(&id).await {
        Err(cellarr_download::DownloadError::NotFound(_)) => {
            println!("LIVE confirmed_gone=true");
        }
        Ok(state) => panic!("torrent {id} still present after remove (state={state:?})"),
        Err(e) => panic!("error confirming removal: {e:?}"),
    }

    println!("LIVE_RESULT=PASS");
}
