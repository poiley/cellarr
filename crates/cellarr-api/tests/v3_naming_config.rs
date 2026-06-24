//! Contract tests for the configurable-naming surface on the `/api/v3` shim:
//! `GET/PUT /config/naming`, `GET /config/naming/tokens`, and
//! `POST /config/naming/preview`.
//!
//! These assert that:
//! * the naming config round-trips through GET/PUT and persists (a later GET sees
//!   the updated formats);
//! * a PUT of an invalid format is rejected with `400` and does not persist;
//! * the token vocabulary endpoint advertises the per-target tokens with their
//!   required/optional markers;
//! * the preview endpoint renders a sample against a candidate format, honoring
//!   required-token strictness and graceful optional-token behavior.

mod common;

use common::start_open;
use serde_json::{json, Value};

#[tokio::test]
async fn naming_config_defaults_then_round_trips_through_put() {
    let server = start_open().await;
    let client = server.client();

    // Defaults: the built-in *arr-conventional formats.
    let cfg: Value = client
        .get(server.url("/api/v3/config/naming"))
        .send()
        .await
        .expect("get naming")
        .json()
        .await
        .expect("naming json");
    assert_eq!(
        cfg["movieFileFormat"],
        "{Movie Title} ({Release Year})/{Movie Title}.{Extension}"
    );
    assert_eq!(cfg["seasonFolderFormat"], "Season {Season}");
    assert_eq!(cfg["seasonFolders"], json!(true));

    // Update two formats; the others are left untouched (partial merge).
    let updated: Value = client
        .put(server.url("/api/v3/config/naming"))
        .json(&json!({
            "movieFileFormat": "{Movie Title}/{Movie Title} ({Release Year}).{Extension}",
            "seasonFolderFormat": "S{Season:00}",
        }))
        .send()
        .await
        .expect("put naming")
        .json()
        .await
        .expect("put json");
    assert_eq!(
        updated["movieFileFormat"],
        "{Movie Title}/{Movie Title} ({Release Year}).{Extension}"
    );
    assert_eq!(updated["seasonFolderFormat"], "S{Season:00}");
    // The episode format was not in the PUT, so it kept its default.
    assert_eq!(
        updated["episodeFileFormat"],
        "{Series Title} - S{Season}E{Episode}.{Extension}"
    );

    // A fresh GET sees the persisted change.
    let after: Value = client
        .get(server.url("/api/v3/config/naming"))
        .send()
        .await
        .expect("get naming again")
        .json()
        .await
        .expect("naming json");
    assert_eq!(after["seasonFolderFormat"], "S{Season:00}");
}

#[tokio::test]
async fn put_rejects_an_invalid_format_and_does_not_persist() {
    let server = start_open().await;
    let client = server.client();

    // An unterminated token is malformed; the PUT must 400.
    let resp = client
        .put(server.url("/api/v3/config/naming"))
        .json(&json!({ "movieFileFormat": "{Movie Title" }))
        .send()
        .await
        .expect("put naming");
    assert_eq!(resp.status(), 400, "an invalid format is rejected");

    // The stored config is unchanged (still the default).
    let cfg: Value = client
        .get(server.url("/api/v3/config/naming"))
        .send()
        .await
        .expect("get naming")
        .json()
        .await
        .expect("naming json");
    assert_eq!(
        cfg["movieFileFormat"],
        "{Movie Title} ({Release Year})/{Movie Title}.{Extension}"
    );
}

#[tokio::test]
async fn tokens_endpoint_advertises_required_and_optional_tokens() {
    let server = start_open().await;
    let client = server.client();

    let tokens: Value = client
        .get(server.url("/api/v3/config/naming/tokens"))
        .send()
        .await
        .expect("get tokens")
        .json()
        .await
        .expect("tokens json");

    let targets = tokens["targets"].as_array().expect("targets array");
    let movie = targets
        .iter()
        .find(|t| t["target"] == "movieFile")
        .expect("movieFile target");
    let movie_tokens = movie["tokens"].as_array().expect("token array");

    let title = movie_tokens
        .iter()
        .find(|t| t["name"] == "Movie Title")
        .expect("Movie Title token");
    assert_eq!(title["required"], json!(true));
    assert_eq!(title["token"], "{Movie Title}");

    let year = movie_tokens
        .iter()
        .find(|t| t["name"] == "Release Year")
        .expect("Release Year token");
    assert_eq!(year["required"], json!(false));
}

#[tokio::test]
async fn preview_renders_a_sample_against_a_candidate_format() {
    let server = start_open().await;
    let client = server.client();

    // Default sample context for a movie file.
    let out: Value = client
        .post(server.url("/api/v3/config/naming/preview"))
        .json(&json!({
            "format": "{Movie Title} ({Release Year})/{Movie Title}.{Extension}",
            "mediaType": "movie",
        }))
        .send()
        .await
        .expect("preview")
        .json()
        .await
        .expect("preview json");
    assert_eq!(out["rendered"], "Blade Runner (1982)/Blade Runner.mkv");
    assert_eq!(out["target"], "movieFile");

    // A caller-supplied sample context overrides the built-in examples.
    let custom: Value = client
        .post(server.url("/api/v3/config/naming/preview"))
        .json(&json!({
            "format": "{Series Title}/Season {Season}/{Series Title} - S{Season}E{Episode}.{Extension}",
            "target": "episodeFile",
            "sampleContext": {
                "Series Title": "Severance",
                "Season": "1",
                "Episode": "9",
                "Extension": "mkv",
            },
        }))
        .send()
        .await
        .expect("preview custom")
        .json()
        .await
        .expect("preview json");
    assert_eq!(
        custom["rendered"],
        "Severance/Season 1/Severance - S1E9.mkv"
    );
}

#[tokio::test]
async fn preview_enforces_required_tokens() {
    let server = start_open().await;
    let client = server.client();

    // A movie preview that references {Series Title} (not in the movie sample)
    // must 400 — the same required-token strictness as the import-time engine.
    let resp = client
        .post(server.url("/api/v3/config/naming/preview"))
        .json(&json!({
            "format": "{Series Title}.{Extension}",
            "mediaType": "movie",
        }))
        .send()
        .await
        .expect("preview");
    assert_eq!(resp.status(), 400, "a missing required token is rejected");
}

#[tokio::test]
async fn media_management_blob_round_trips_and_merges_per_card() {
    let server = start_open().await;
    let client = server.client();

    // Defaults: naming present, extras disabled, no permission policy.
    let mm: Value = client
        .get(server.url("/api/v3/config/mediamanagement"))
        .send()
        .await
        .expect("get mediamanagement")
        .json()
        .await
        .expect("mm json");
    assert_eq!(
        mm["naming"]["movieFileFormat"],
        "{Movie Title} ({Release Year})/{Movie Title}.{Extension}"
    );
    assert_eq!(mm["extraFiles"]["enabled"], json!(false));

    // The Permissions card saves only `permissions` — it must not clobber naming.
    let after_perms: Value = client
        .put(server.url("/api/v3/config/mediamanagement"))
        .json(&json!({ "permissions": { "chmodFile": "640", "chmodFolder": "750" } }))
        .send()
        .await
        .expect("put permissions")
        .json()
        .await
        .expect("put json");
    assert_eq!(after_perms["permissions"]["chmodFile"], "640");
    assert_eq!(
        after_perms["naming"]["movieFileFormat"],
        "{Movie Title} ({Release Year})/{Movie Title}.{Extension}",
        "saving permissions left naming untouched"
    );

    // The Extra Files card saves only `extraFiles` — permissions stay persisted.
    let after_extra: Value = client
        .put(server.url("/api/v3/config/mediamanagement"))
        .json(&json!({ "extraFiles": { "enabled": true, "extensions": ["srt", "nfo"] } }))
        .send()
        .await
        .expect("put extra")
        .json()
        .await
        .expect("put json");
    assert_eq!(after_extra["extraFiles"]["enabled"], json!(true));
    assert_eq!(
        after_extra["permissions"]["chmodFile"], "640",
        "saving extras kept the earlier permissions"
    );

    // A fresh GET sees both persisted independently.
    let final_mm: Value = client
        .get(server.url("/api/v3/config/mediamanagement"))
        .send()
        .await
        .expect("get mediamanagement again")
        .json()
        .await
        .expect("mm json");
    assert_eq!(final_mm["permissions"]["chmodFolder"], "750");
    assert_eq!(final_mm["extraFiles"]["extensions"][0], "srt");
}

#[tokio::test]
async fn media_management_put_rejects_an_invalid_naming_format() {
    let server = start_open().await;
    let client = server.client();

    let resp = client
        .put(server.url("/api/v3/config/mediamanagement"))
        .json(&json!({
            "naming": {
                "movieFileFormat": "{Movie Title",
                "seriesFolderFormat": "{Series Title}",
                "seasonFolderFormat": "Season {Season}",
                "episodeFileFormat": "{Series Title} - S{Season}E{Episode}.{Extension}",
            }
        }))
        .send()
        .await
        .expect("put mediamanagement");
    assert_eq!(resp.status(), 400, "an invalid naming format is rejected");
}
