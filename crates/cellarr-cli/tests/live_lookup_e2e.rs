//! Opt-in **live** end-to-end metadata lookup through the daemon.
//!
//! Boots a real daemon with the TheTVDB credentials (from the env, mirroring the
//! daemon's config keys), then calls the **Sonarr-face** `series/lookup` over HTTP
//! and asserts it resolves a real series — the correct `tvdbId` (81189 for
//! Breaking Bad) and a human title, not the search term echoed back or a UUID.
//!
//! This is the Phase E acceptance test: it proves the live `cellarr-meta` source
//! is wired through `AppState` into the v3 shim end to end, over the real network.
//!
//! Gating: like the `cellarr-meta` live smoke test, this **self-skips** (printing
//! why) when `CELLARR_TVDB__API_KEY` is absent, so a normal `cargo test` never
//! touches the network. Run it with the key in scope:
//!   `set -a; . ./.env; set +a && \`
//!   `  cargo test -p cellarr-cli --test live_lookup_e2e -- --nocapture`

use cellarr_cli::boot::Daemon;
use cellarr_cli::config::{Config, TvdbConfig};

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

#[tokio::test]
async fn live_sonarr_face_series_lookup_resolves_breaking_bad() {
    let Some(api_key) = env_nonempty("CELLARR_TVDB__API_KEY") else {
        eprintln!(
            "skipping live lookup e2e: CELLARR_TVDB__API_KEY not set \
             (`set -a; . ./.env; set +a` to enable)"
        );
        return;
    };
    let pin = env_nonempty("CELLARR_TVDB__PIN");

    // Boot a hermetic daemon (tempdir data dir, OS-assigned port) carrying the
    // live TheTVDB credentials — exactly the production wiring path.
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = Config {
        data_dir: dir.path().to_path_buf(),
        ..Config::default()
    };
    config.api.port = 0;
    config.tvdb = TvdbConfig {
        api_key: Some(api_key),
        pin,
    };

    let daemon = Daemon::boot(&config).await.expect("daemon boots");
    let addr = daemon.addr();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let serve = tokio::spawn(async move {
        daemon
            .serve_until(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/sonarr/api/v3/series/lookup?term=Breaking%20Bad");

    // Wait for the server to accept, then call the live lookup.
    let resp = {
        let mut last = None;
        for _ in 0..50 {
            match client.get(&url).send().await {
                Ok(r) => {
                    last = Some(r);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
            }
        }
        last.expect("server became reachable")
    };
    assert_eq!(resp.status(), 200, "live lookup should return 200");
    let body: Vec<serde_json::Value> = resp.json().await.expect("json array");

    assert!(
        !body.is_empty(),
        "live lookup returned no candidates for 'Breaking Bad'"
    );

    // Find the Breaking Bad candidate and assert the real id + title. We do NOT
    // accept the first row blindly: we require a row whose tvdbId is 81189 AND
    // whose title is the real series name (guards the fake-green patterns of
    // echoing the term or hardcoding an id).
    let bb = body
        .iter()
        .find(|c| c.get("tvdbId").and_then(serde_json::Value::as_i64) == Some(81189))
        .unwrap_or_else(|| panic!("no candidate with tvdbId 81189 in live results: {body:?}"));
    let title = bb
        .get("title")
        .and_then(serde_json::Value::as_str)
        .expect("candidate has a title");
    assert!(
        title.contains("Breaking Bad"),
        "expected the human title 'Breaking Bad', got {title:?}"
    );
    // The title must never be the echoed term variant or a UUID.
    assert_ne!(
        title, "Breaking%20Bad",
        "title must be resolved, not the URL-encoded term"
    );

    eprintln!("LIVE e2e OK: Sonarr-face lookup resolved tvdbId=81189 title={title:?}");

    shutdown_tx.send(()).expect("send shutdown");
    serve.await.expect("serve joins").expect("clean shutdown");
}
