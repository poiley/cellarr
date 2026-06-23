//! Opt-in **live** TheTVDB v4 smoke test (drift detection against the real API).
//!
//! Unlike the record/replay suite (which runs over a [`RecordedFetcher`] and is
//! the CI contract), this test hits the real `api4.thetvdb.com` over a live
//! [`ReqwestFetcher`]. It is gated on a credential being present in the
//! environment, so a normal `cargo test` run **self-skips** it (printing why) and
//! never touches the network — keeping the spec's "no live source on the CI
//! critical path" rule intact while still letting an operator verify the live
//! login + fetch path with a real key.
//!
//! Run it with the key in scope (the gitignored `.env`):
//!   `set -a; . ./.env; set +a && cargo test -p cellarr-meta --test live_thetvdb -- --nocapture`
//!
//! The env vars mirror the daemon's config keys exactly:
//!   - `CELLARR_TVDB__API_KEY` (required to run; absent → skip)
//!   - `CELLARR_TVDB__PIN`     (optional subscriber pin for the user-supported model)

use cellarr_meta::{ReqwestFetcher, TheTvdbConfig, TheTvdbSource};

/// Breaking Bad, per the task brief (a stable, well-populated series).
const BREAKING_BAD_TVDB_ID: &str = "81189";

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

#[tokio::test]
async fn live_login_and_fetch_breaking_bad() {
    let Some(api_key) = env_nonempty("CELLARR_TVDB__API_KEY") else {
        eprintln!(
            "skipping live TheTVDB test: CELLARR_TVDB__API_KEY not set \
             (`set -a; . ./.env; set +a` to enable)"
        );
        return;
    };
    let pin = env_nonempty("CELLARR_TVDB__PIN");

    let config = TheTvdbConfig {
        api_key: Some(api_key),
        pin,
        ..TheTvdbConfig::default()
    };
    let source = TheTvdbSource::new(ReqwestFetcher::new("thetvdb"), config);

    let meta = match source.fetch_normalized(BREAKING_BAD_TVDB_ID).await {
        Ok(m) => m,
        Err(e) => {
            // A user-supplied (non-licensed) key typically requires a subscriber
            // PIN at login; without one TheTVDB answers 401. Surface that as a
            // clear, non-secret-leaking message rather than an opaque assert.
            panic!(
                "live TheTVDB fetch failed ({e}). If this is a user-supported key, \
                 a CELLARR_TVDB__PIN is likely required."
            );
        }
    };

    // Title, year, and episode-count sanity (the task's acceptance criteria).
    assert!(
        meta.title.contains("Breaking Bad"),
        "expected title to contain 'Breaking Bad', got {:?}",
        meta.title
    );
    let year = meta.year.expect("series should have a year");
    assert!(
        (2000..=2030).contains(&year),
        "expected a sane year, got {year}"
    );
    assert!(
        !meta.children.is_empty(),
        "expected >0 episodes, got {}",
        meta.children.len()
    );

    eprintln!(
        "LIVE TheTVDB OK: title={:?} year={year} episodes={} external_ids={:?}",
        meta.title,
        meta.children.len(),
        meta.external_ids
    );

    // Search by name should find Breaking Bad among the candidates (same login
    // token is reused — exercises the search path live too).
    let results = source
        .search_normalized("Breaking Bad")
        .await
        .expect("live search should succeed");
    assert!(
        results.iter().any(|r| r.title.contains("Breaking Bad")),
        "expected a 'Breaking Bad' candidate in live search results"
    );
    eprintln!("LIVE TheTVDB search returned {} candidates", results.len());
}
