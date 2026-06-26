//! Contract tests for the custom-format authoring surface and the delay-profile
//! CRUD on the `/api/v3` shim.
//!
//! These assert that:
//! * a custom format authored through `POST /customformat` with several typed
//!   specification kinds (Source / Resolution / ReleaseGroup) persists and is then
//!   used by the **same** matcher the decision engine uses,
//! * `POST /customformat/test` reports which stored formats match a release title,
//! * delay profiles round-trip through their CRUD, and
//! * the schema enumerates the available specification implementations.

mod common;

use cellarr_core::repo::ProfileRepository;
use common::start_open;
use serde_json::{json, Value};

// --- custom-format authoring -----------------------------------------------

#[tokio::test]
async fn customformat_schema_enumerates_implementations() {
    let server = start_open().await;
    let client = server.client();
    let schema: Value = client
        .get(server.url("/api/v3/customformat/schema"))
        .send()
        .await
        .expect("schema request")
        .json()
        .await
        .expect("schema json");
    let names: Vec<&str> = schema
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|s| s["implementation"].as_str())
        .collect();
    // Every implementation the editor can build a form for must be present.
    for expected in [
        "ReleaseTitleSpecification",
        "ReleaseGroupSpecification",
        "SourceSpecification",
        "ResolutionSpecification",
        "QualityModifierSpecification",
        "ReleaseTypeSpecification",
        "LanguageSpecification",
        "IndexerFlagSpecification",
        "SizeSpecification",
    ] {
        assert!(names.contains(&expected), "schema missing {expected}");
    }
    // The Source spec is a select carrying the source tokens as options.
    let source = schema
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["implementation"] == "SourceSpecification")
        .expect("source spec");
    let opts = source["fields"][0]["selectOptions"]
        .as_array()
        .expect("select options");
    let tokens: Vec<&str> = opts.iter().filter_map(|o| o["value"].as_str()).collect();
    assert!(tokens.contains(&"web-dl"), "source options include web-dl");
    assert!(tokens.contains(&"bluray"), "source options include bluray");
}

#[tokio::test]
async fn customformat_create_persists_typed_specs_and_matcher_matches() {
    use cellarr_decide::MatchContext;
    let server = start_open().await;
    let client = server.client();

    // Author a CF with THREE typed spec kinds: required Source=web-dl,
    // required Resolution=1080p, and a non-required ReleaseGroup regex.
    let created: Value = client
        .post(server.url("/api/v3/customformat"))
        .json(&json!({
            "name": "WEB 1080p GoodGroup",
            "specifications": [
                { "name": "src", "implementation": "SourceSpecification",
                  "negate": false, "required": true,
                  "fields": [ { "name": "value", "value": "web-dl" } ] },
                { "name": "res", "implementation": "ResolutionSpecification",
                  "negate": false, "required": true,
                  "fields": [ { "name": "value", "value": "1080p" } ] },
                { "name": "grp", "implementation": "ReleaseGroupSpecification",
                  "negate": false, "required": false,
                  "fields": [ { "name": "value", "value": "(GoodGroup)" } ] },
            ],
        }))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    assert_eq!(created["name"], "WEB 1080p GoodGroup");
    // The persisted specs round-trip with their typed implementation + value.
    let specs = created["specifications"].as_array().expect("specs");
    assert_eq!(specs.len(), 3);
    let impls: Vec<&str> = specs
        .iter()
        .filter_map(|s| s["implementation"].as_str())
        .collect();
    assert!(impls.contains(&"SourceSpecification"));
    assert!(impls.contains(&"ResolutionSpecification"));
    assert!(impls.contains(&"ReleaseGroupSpecification"));

    // Load the persisted CF and feed it to the SAME matcher the decision engine
    // uses. A WEB-DL 1080p GoodGroup release matches; a 720p one does not (the
    // required Resolution fails).
    let formats = server
        .state
        .db
        .profiles()
        .custom_formats()
        .await
        .expect("custom_formats");
    assert_eq!(formats.len(), 1);
    let ctx = MatchContext::new(&formats).expect("compiles");

    let mk = |title: &str,
              source: cellarr_core::Source,
              resolution: cellarr_core::Resolution,
              group: &str| {
        let mut parsed = cellarr_core::ParsedRelease::new(title);
        parsed.source = Some(source);
        parsed.resolution = Some(resolution);
        parsed.group = Some(group.to_string());
        let release = cellarr_core::Release {
            indexer_id: cellarr_core::IndexerId::new(),
            title: title.to_string(),
            download_url: String::new(),
            guid: None,
            protocol: cellarr_core::Protocol::Torrent,
            size: None,
            seeders: None,
            indexer_flags: vec![],
        };
        (release, parsed)
    };

    let (r_ok, p_ok) = mk(
        "Show.S01E01.1080p.WEB-DL.x264-GoodGroup",
        cellarr_core::Source::WebDl,
        cellarr_core::Resolution::R1080p,
        "GoodGroup",
    );
    assert!(
        ctx.matches(&formats[0], &r_ok, &p_ok),
        "WEB-DL 1080p GoodGroup must match the authored CF"
    );

    let (r_no, p_no) = mk(
        "Show.S01E01.720p.WEB-DL.x264-GoodGroup",
        cellarr_core::Source::WebDl,
        cellarr_core::Resolution::R720p,
        "GoodGroup",
    );
    assert!(
        !ctx.matches(&formats[0], &r_no, &p_no),
        "720p must NOT match (required Resolution=1080p fails)"
    );
}

#[tokio::test]
async fn customformat_update_and_delete_roundtrip() {
    let server = start_open().await;
    let client = server.client();

    let created: Value = client
        .post(server.url("/api/v3/customformat"))
        .json(&json!({
            "name": "Original",
            "specifications": [
                { "name": "s", "implementation": "ReleaseTitleSpecification",
                  "required": true, "negate": false,
                  "fields": [ { "name": "value", "value": "proper" } ] },
            ],
        }))
        .send()
        .await
        .expect("create")
        .json()
        .await
        .expect("create json");
    let id = created["id"].as_i64().expect("numeric id");

    // GET by id returns the same format.
    let got: Value = client
        .get(server.url(&format!("/api/v3/customformat/{id}")))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .expect("get json");
    assert_eq!(got["id"], id);
    assert_eq!(got["name"], "Original");

    // UPDATE renames and swaps the spec to a typed Source.
    let updated: Value = client
        .put(server.url(&format!("/api/v3/customformat/{id}")))
        .json(&json!({
            "name": "Renamed",
            "specifications": [
                { "name": "src", "implementation": "SourceSpecification",
                  "required": false, "negate": false,
                  "fields": [ { "name": "value", "value": "bluray" } ] },
            ],
        }))
        .send()
        .await
        .expect("update")
        .json()
        .await
        .expect("update json");
    assert_eq!(updated["id"], id, "id is preserved across update");
    assert_eq!(updated["name"], "Renamed");
    assert_eq!(
        updated["specifications"][0]["implementation"],
        "SourceSpecification"
    );

    // The store reflects the update (one format, the new condition).
    let formats = server
        .state
        .db
        .profiles()
        .custom_formats()
        .await
        .expect("custom_formats");
    assert_eq!(formats.len(), 1);
    assert_eq!(formats[0].name, "Renamed");
    assert!(matches!(
        formats[0].conditions[0].kind,
        cellarr_core::ConditionKind::Source {
            source: cellarr_core::Source::Bluray
        }
    ));

    // DELETE actually removes it, and is idempotent.
    let del = client
        .delete(server.url(&format!("/api/v3/customformat/{id}")))
        .send()
        .await
        .expect("delete");
    assert_eq!(del.status(), 200);
    assert!(server
        .state
        .db
        .profiles()
        .custom_formats()
        .await
        .expect("custom_formats")
        .is_empty());
    let del2 = client
        .delete(server.url(&format!("/api/v3/customformat/{id}")))
        .send()
        .await
        .expect("delete 2");
    assert_eq!(del2.status(), 200);
}

#[tokio::test]
async fn customformat_test_reports_matches() {
    let server = start_open().await;
    let client = server.client();

    // Two CFs: one that matches "PROPER" titles, one matching a HEVC release.
    client
        .post(server.url("/api/v3/customformat"))
        .json(&json!({
            "name": "Proper",
            "specifications": [
                { "name": "s", "implementation": "ReleaseTitleSpecification",
                  "required": true, "negate": false,
                  "fields": [ { "name": "value", "value": "\\bproper\\b" } ] },
            ],
        }))
        .send()
        .await
        .expect("create proper");
    client
        .post(server.url("/api/v3/customformat"))
        .json(&json!({
            "name": "HEVC",
            "specifications": [
                { "name": "s", "implementation": "ReleaseTitleSpecification",
                  "required": true, "negate": false,
                  "fields": [ { "name": "value", "value": "(x265|hevc)" } ] },
            ],
        }))
        .send()
        .await
        .expect("create hevc");

    // A PROPER (non-HEVC) title: Proper matches, HEVC does not.
    let report: Value = client
        .post(server.url("/api/v3/customformat/test"))
        .json(&json!({ "title": "Show.S01E01.PROPER.1080p.WEB-DL.x264-GRP" }))
        .send()
        .await
        .expect("test request")
        .json()
        .await
        .expect("test json");
    let entries = report.as_array().expect("array");
    let matched: std::collections::HashMap<&str, bool> = entries
        .iter()
        .map(|e| (e["name"].as_str().unwrap(), e["matched"].as_bool().unwrap()))
        .collect();
    assert_eq!(matched.get("Proper"), Some(&true), "PROPER title matches");
    assert_eq!(matched.get("HEVC"), Some(&false), "non-HEVC title does not");
}

#[tokio::test]
async fn customformat_test_honors_parsed_overrides() {
    let server = start_open().await;
    let client = server.client();
    // A typed Source CF; the title carries no source token, so we feed it via the
    // parsed override the editor's live preview supplies.
    client
        .post(server.url("/api/v3/customformat"))
        .json(&json!({
            "name": "WebDl",
            "specifications": [
                { "name": "src", "implementation": "SourceSpecification",
                  "required": true, "negate": false,
                  "fields": [ { "name": "value", "value": "web-dl" } ] },
            ],
        }))
        .send()
        .await
        .expect("create");
    let report: Value = client
        .post(server.url("/api/v3/customformat/test"))
        .json(&json!({
            "title": "Some.Show.Title",
            "parsed": { "source": "web-dl" },
        }))
        .send()
        .await
        .expect("test")
        .json()
        .await
        .expect("test json");
    let entry = &report.as_array().unwrap()[0];
    assert_eq!(entry["name"], "WebDl");
    assert_eq!(entry["matched"], true, "override source makes it match");
}

// --- delay-profile CRUD ----------------------------------------------------

#[tokio::test]
async fn delayprofile_create_list_update_delete_roundtrip() {
    let server = start_open().await;
    let client = server.client();

    let created: Value = client
        .post(server.url("/api/v3/delayprofile"))
        .json(&json!({
            "enableUsenet": true,
            "enableTorrent": true,
            "preferredProtocol": "usenet",
            "usenetDelay": 30,
            "torrentDelay": 60,
            "bypassIfHighestQuality": true,
            "tags": ["anime"],
            "order": 1,
        }))
        .send()
        .await
        .expect("create")
        .json()
        .await
        .expect("create json");
    assert_eq!(created["preferredProtocol"], "usenet");
    assert_eq!(created["usenetDelay"], 30);
    assert_eq!(created["torrentDelay"], 60);
    assert_eq!(created["bypassIfHighestQuality"], true);
    let id = created["id"].as_i64().expect("numeric id");

    // LIST shows it.
    let list: Value = client
        .get(server.url("/api/v3/delayprofile"))
        .send()
        .await
        .expect("list")
        .json()
        .await
        .expect("list json");
    let arr = list.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], id);

    // The persisted profile carries the typed fields.
    let stored = server
        .state
        .db
        .profiles()
        .list_delay_profiles()
        .await
        .expect("list_delay_profiles");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].usenet_delay, 30);
    assert_eq!(stored[0].torrent_delay, 60);
    assert!(stored[0].bypass_if_highest_quality);
    assert_eq!(stored[0].tags, vec!["anime".to_string()]);
    assert_eq!(
        stored[0].preferred_protocol,
        cellarr_core::PreferredProtocol::Usenet
    );

    // UPDATE: switch preference, drop the bypass, change the delays.
    let updated: Value = client
        .put(server.url(&format!("/api/v3/delayprofile/{id}")))
        .json(&json!({
            "preferredProtocol": "torrent",
            "usenetDelay": 0,
            "torrentDelay": 15,
            "bypassIfHighestQuality": false,
            "tags": [],
            "order": 2,
        }))
        .send()
        .await
        .expect("update")
        .json()
        .await
        .expect("update json");
    assert_eq!(updated["id"], id);
    assert_eq!(updated["preferredProtocol"], "torrent");
    assert_eq!(updated["torrentDelay"], 15);

    let stored = server
        .state
        .db
        .profiles()
        .list_delay_profiles()
        .await
        .expect("list_delay_profiles");
    assert_eq!(stored[0].torrent_delay, 15);
    assert_eq!(stored[0].usenet_delay, 0);
    assert!(!stored[0].bypass_if_highest_quality);
    assert_eq!(
        stored[0].preferred_protocol,
        cellarr_core::PreferredProtocol::Torrent
    );

    // DELETE removes it, idempotently.
    let del = client
        .delete(server.url(&format!("/api/v3/delayprofile/{id}")))
        .send()
        .await
        .expect("delete");
    assert_eq!(del.status(), 200);
    assert!(server
        .state
        .db
        .profiles()
        .list_delay_profiles()
        .await
        .expect("list_delay_profiles")
        .is_empty());
    let del2 = client
        .delete(server.url(&format!("/api/v3/delayprofile/{id}")))
        .send()
        .await
        .expect("delete 2");
    assert_eq!(del2.status(), 200);
}

// --- release-profile CRUD + schema -----------------------------------------

#[tokio::test]
async fn releaseprofile_schema_lists_the_spec() {
    let server = start_open().await;
    let client = server.client();
    let schema: Value = client
        .get(server.url("/api/v3/releaseprofile/schema"))
        .send()
        .await
        .expect("schema request")
        .json()
        .await
        .expect("schema json");
    // The schema spells out the editable fields (required / ignored / preferred /
    // enabled / tags) so a client can build the form.
    let names: Vec<&str> = schema["fields"]
        .as_array()
        .expect("fields array")
        .iter()
        .filter_map(|f| f["name"].as_str())
        .collect();
    for expected in [
        "name",
        "enabled",
        "required",
        "ignored",
        "preferred",
        "tags",
    ] {
        assert!(names.contains(&expected), "schema missing field {expected}");
    }
    // The empty-form defaults are present.
    assert!(schema["required"].is_array());
    assert!(schema["ignored"].is_array());
    assert!(schema["preferred"].is_array());
}

#[tokio::test]
async fn releaseprofile_create_list_update_delete_roundtrip() {
    let server = start_open().await;
    let client = server.client();

    let created: Value = client
        .post(server.url("/api/v3/releaseProfile"))
        .json(&json!({
            "name": "anime",
            "enabled": true,
            "required": ["bluray", "/x26[45]/"],
            "ignored": ["cam"],
            "preferred": [
                { "key": "remux", "value": 100 },
                { "key": "/atmos/", "value": -25 },
            ],
            "tags": [3, 7],
        }))
        .send()
        .await
        .expect("create")
        .json()
        .await
        .expect("create json");
    assert_eq!(created["name"], "anime");
    assert_eq!(created["enabled"], true);
    assert_eq!(created["required"], json!(["bluray", "/x26[45]/"]));
    assert_eq!(created["ignored"], json!(["cam"]));
    assert_eq!(
        created["preferred"],
        json!([
            { "key": "remux", "value": 100 },
            { "key": "/atmos/", "value": -25 },
        ])
    );
    assert_eq!(created["tags"], json!([3, 7]));
    let id = created["id"].as_i64().expect("numeric id");
    // The id is JS-safe.
    assert!(id <= 9_007_199_254_740_991, "id must be JS-safe");

    // LIST shows it.
    let list: Value = client
        .get(server.url("/api/v3/releaseprofile"))
        .send()
        .await
        .expect("list")
        .json()
        .await
        .expect("list json");
    let arr = list.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], id);

    // GET by id.
    let got: Value = client
        .get(server.url(&format!("/api/v3/releaseprofile/{id}")))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .expect("get json");
    assert_eq!(got["id"], id);
    assert_eq!(got["name"], "anime");

    // The persisted profile carries the typed fields (tag ids, terms, scores).
    let stored = server
        .state
        .db
        .profiles()
        .list_release_profiles()
        .await
        .expect("list_release_profiles");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].tags, vec![3, 7]);
    assert_eq!(stored[0].ignored, vec!["cam".to_string()]);
    assert_eq!(stored[0].preferred[0].term, "remux");
    assert_eq!(stored[0].preferred[0].score, 100);
    assert_eq!(stored[0].preferred[1].score, -25);

    // UPDATE: rename, disable, swap the term lists.
    let updated: Value = client
        .put(server.url(&format!("/api/v3/releaseprofile/{id}")))
        .json(&json!({
            "name": "anime-v2",
            "enabled": false,
            "required": [],
            "ignored": ["x265"],
            "preferred": [],
            "tags": [],
        }))
        .send()
        .await
        .expect("update")
        .json()
        .await
        .expect("update json");
    assert_eq!(updated["id"], id);
    assert_eq!(updated["name"], "anime-v2");
    assert_eq!(updated["enabled"], false);
    assert_eq!(updated["ignored"], json!(["x265"]));

    let stored = server
        .state
        .db
        .profiles()
        .list_release_profiles()
        .await
        .expect("list_release_profiles");
    assert!(!stored[0].enabled);
    assert_eq!(stored[0].ignored, vec!["x265".to_string()]);
    assert!(stored[0].preferred.is_empty());
    assert!(stored[0].tags.is_empty());

    // DELETE removes it, idempotently.
    let del = client
        .delete(server.url(&format!("/api/v3/releaseprofile/{id}")))
        .send()
        .await
        .expect("delete");
    assert_eq!(del.status(), 200);
    assert!(server
        .state
        .db
        .profiles()
        .list_release_profiles()
        .await
        .expect("list_release_profiles")
        .is_empty());
    let del2 = client
        .delete(server.url(&format!("/api/v3/releaseprofile/{id}")))
        .send()
        .await
        .expect("delete 2");
    assert_eq!(del2.status(), 200);
}
