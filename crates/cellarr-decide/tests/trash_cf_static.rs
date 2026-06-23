//! Static (no live app) counterpart to the `oracle_trash_cf` oracle.
//!
//! The oracle proves CF-matching parity against a live Sonarr/Radarr; this test
//! pins the *mechanical* behaviors the oracle established — implementation-grouped
//! boolean algebra, ReleaseGroup-as-regex, and app-specific Source enum indices —
//! against the **real** TRaSH fixtures, so a regression is caught in the fast
//! hermetic suite (no Docker, no network) and not only when the oracle is run.
//!
//! Anchor expectations below were each verified against the live apps' `/api/v3/
//! parse` while building the oracle; they encode the non-obvious truths the
//! mechanical fixes were derived from.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use cellarr_core::{IndexerId, Protocol, Release};
use cellarr_decide::{import_trash_custom_formats_counted_for_app, MatchContext, TrashApp};
use cellarr_parse::parse_title;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/trash")
}

fn load_cf_json(app: &str) -> (String, std::collections::HashMap<String, i32>) {
    let base = fixtures_dir().join(app);
    let cf_dir = base.join("cf");
    let mut files: Vec<PathBuf> = fs::read_dir(&cf_dir)
        .unwrap_or_else(|e| panic!("read {cf_dir:?}: {e}"))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let arr: Vec<serde_json::Value> = files
        .iter()
        .map(|p| {
            let t = fs::read_to_string(p).unwrap();
            serde_json::from_str(&t).unwrap()
        })
        .collect();
    let scores: std::collections::HashMap<String, i32> = {
        let t = fs::read_to_string(base.join("scores.default.json")).unwrap();
        serde_json::from_str(&t).unwrap()
    };
    (serde_json::to_string(&arr).unwrap(), scores)
}

fn release_for(title: &str) -> Release {
    Release {
        indexer_id: IndexerId::new(),
        title: title.to_string(),
        download_url: String::new(),
        guid: None,
        protocol: Protocol::Torrent,
        size: None,
        seeders: None,
        indexer_flags: vec![],
    }
}

struct Matcher {
    formats: Vec<cellarr_core::CustomFormat>,
}

impl Matcher {
    fn for_app(app: &str, trash_app: TrashApp) -> Self {
        let (json, scores) = load_cf_json(app);
        let report =
            import_trash_custom_formats_counted_for_app(&json, &scores, trash_app).expect("import");
        Matcher {
            formats: report.formats,
        }
    }

    fn matched(&self, title: &str) -> BTreeSet<String> {
        let ctx = MatchContext::new(&self.formats).expect("context");
        let parsed = parse_title(title);
        let rel = release_for(title);
        self.formats
            .iter()
            .filter(|f| ctx.matches(f, &rel, &parsed))
            .map(|f| f.name.clone())
            .collect()
    }
}

#[test]
fn implementation_grouping_prevents_tier_over_matching_sonarr() {
    // A WEB-DL anime episode whose group is NOT in any anime tier's list must not
    // match ANY "Anime Web Tier NN" CF. The old flat-OR matched all of them via
    // the bare Source=web condition; the grouped algebra requires the
    // ReleaseTitle group to ALSO match. (Verified live.)
    let m = Matcher::for_app("sonarr", TrashApp::Sonarr);
    let got = m.matched("[SubsPlease] Show Title - 12 (1080p) [ABCD1234].mkv");
    let tiers: Vec<&String> = got
        .iter()
        .filter(|n| n.starts_with("Anime Web Tier"))
        .collect();
    assert!(
        tiers.is_empty(),
        "no Anime Web Tier CF must match a WEB anime release with an unlisted group; got {tiers:?}"
    );
}

#[test]
fn release_group_regex_no_rlsgroup_only_when_groupless() {
    // No-RlsGroup is ReleaseGroup `.` negated: matches ONLY a release with no
    // group. A grouped release must NOT match it. (Verified live.)
    let m = Matcher::for_app("sonarr", TrashApp::Sonarr);
    let with_group = m.matched("Show.Title.S01E01.1080p.WEB-DL.x264-SOMEGROUP");
    assert!(
        !with_group.contains("No-RlsGroup"),
        "a grouped release must not match No-RlsGroup"
    );
    let no_group = m.matched("Show.Title.S01E01.1080p.WEB-DL.x264");
    assert!(
        no_group.contains("No-RlsGroup"),
        "a groupless release must match No-RlsGroup; got {no_group:?}"
    );
}

#[test]
fn radarr_source_dialect_matches_webdl_tier() {
    // Radarr's source enum differs from Sonarr's; a WEB-DL movie should engage
    // the WEB tier/source-conditioned CFs (the dialect maps index 7 -> WEB-DL).
    // We assert at least one WEB-flavored CF matches a clean WEB-DL movie, which
    // is impossible if index 7 were mis-mapped (it would read as Bluray/DVD).
    let m = Matcher::for_app("radarr", TrashApp::Radarr);
    let got = m.matched("Some.Movie.2021.1080p.AMZN.WEB-DL.DDP5.1.H.264-FLUX");
    assert!(
        got.iter()
            .any(|n| n.contains("WEB") || n == "AMZN" || n == "FLUX"),
        "a WEB-DL movie must engage WEB-dialect CFs; got {got:?}"
    );
}

#[test]
fn both_fixture_sets_build_a_match_context() {
    // Importing the real sets with the correct dialect and matching a handful of
    // titles must never panic (regex compile, grouping). A smoke test over the
    // whole supported set.
    for (app, trash) in [("sonarr", TrashApp::Sonarr), ("radarr", TrashApp::Radarr)] {
        let m = Matcher::for_app(app, trash);
        assert!(
            m.formats.len() > 200,
            "{app}: expected the full supported set"
        );
        let _ = m.matched("Test.Title.2020.1080p.BluRay.x264-GROUP");
        let _ = m.matched("[Group] Anime - 01 [1080p].mkv");
    }
}

// Keep the import path honest: the fixtures dir must exist and be non-empty.
#[test]
fn fixtures_present() {
    let p: &Path = &fixtures_dir();
    assert!(p.join("sonarr/cf").is_dir(), "sonarr CF fixtures missing");
    assert!(p.join("radarr/cf").is_dir(), "radarr CF fixtures missing");
}
