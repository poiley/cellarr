//! Import the **real** TRaSH-Guides custom-format sets and assert cellarr can
//! consume them at a high, ratcheted supported-rate — and that every supported
//! CF compiles into a valid [`MatchContext`] (its title regexes are accepted by
//! cellarr's regex engine).
//!
//! The fixtures under `tests/fixtures/trash/{sonarr,radarr}/cf/*.json` are a
//! curated copy of TRaSH-Guides/Guides' published CF JSON (see
//! `tests/fixtures/trash/SOURCE.md` for provenance and terms). They are *test
//! data*, attributed, not a relicensed part of cellarr.
//!
//! Unsupported `implementation`s are **skipped and counted** by
//! `import_trash_custom_formats_counted`, never silently dropped: this test
//! prints the per-implementation skip tally so a regression that drops support
//! for a whole spec kind shows up as a falling rate.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use cellarr_decide::{import_trash_custom_formats_counted, MatchContext, TrashImportReport};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/trash")
}

/// Concatenate every CF JSON file in `cf_dir` into a single JSON array, matching
/// the array shape the importer expects (TRaSH publishes one CF per file).
fn load_cf_array(cf_dir: &Path) -> Vec<serde_json::Value> {
    let mut files: Vec<PathBuf> = fs::read_dir(cf_dir)
        .unwrap_or_else(|e| panic!("read {cf_dir:?}: {e}"))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no CF fixtures under {cf_dir:?}");
    files
        .iter()
        .map(|p| {
            let text = fs::read_to_string(p).unwrap_or_else(|e| panic!("read {p:?}: {e}"));
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {p:?}: {e}"))
        })
        .collect()
}

/// Load the derived `trash_id -> recommended score` map (the upstream `default`
/// flavor; 0 where a CF had no default score).
fn load_scores(path: &Path) -> HashMap<String, i32> {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path:?}: {e}"))
}

fn import_app(app: &str) -> TrashImportReport {
    let base = fixtures_dir().join(app);
    let cfs = load_cf_array(&base.join("cf"));
    let json = serde_json::to_string(&cfs).expect("re-serialize CF array");
    let scores = load_scores(&base.join("scores.default.json"));
    import_trash_custom_formats_counted(&json, &scores).expect("import (tolerant) succeeds")
}

/// Print a per-app summary and return the report for assertions.
fn report_for(app: &str) -> TrashImportReport {
    let report = import_app(app);
    println!(
        "\n=== TRaSH {app} CF import ===\n  total: {}  supported: {}  rate: {:.4}  skipped: {}",
        report.total,
        report.supported(),
        report.supported_rate(),
        report.skipped.len(),
    );
    if !report.unsupported_counts.is_empty() {
        let mut tally: Vec<_> = report.unsupported_counts.iter().collect();
        tally.sort();
        println!("  unsupported implementations (skipped, counted):");
        for (imp, n) in tally {
            println!("    {imp:32} {n}");
        }
    }
    report
}

/// Building a [`MatchContext`] compiles every release-title regex; if it
/// succeeds, the supported CFs are usable for real matching/scoring decisions.
fn assert_match_context_builds(report: &TrashImportReport, app: &str) {
    let ctx =
        MatchContext::new(&report.formats).unwrap_or_else(|e| panic!("{app}: regex compile: {e}"));
    assert_eq!(
        ctx.formats().len(),
        report.formats.len(),
        "{app}: every supported CF must be present in the match context"
    );
}

// Ratchets: lower bounds on the supported-rate achieved against the curated
// fixtures. Raising support (or upstream dropping a construct cellarr cannot
// model) only moves them up; a drop below means cellarr lost the ability to
// import a class of real-world CFs and must be investigated.
//
// A CF is "supported" when every spec maps to a ConditionKind AND every title
// regex compiles under cellarr's engine. The skipped tail is two classes, both
// counted (never silently dropped):
//   * `ReleaseTypeSpecification` — Sonarr Single/Multi-Episode/Season-Pack
//     release type, which has no field on cellarr's parse to model (3 CFs).
//   * `ReleaseTitleSpecification(incompatible-regex)` — CFs whose title regex
//     uses a .NET-only construct cellarr's fancy-regex engine rejects
//     (variable-size look-behind; `[` inside a character class).
//
// Achieved at the pinned fixture commit:
//   Sonarr 223/235 = 0.9489 (9 incompatible-regex + 3 ReleaseType)
//   Radarr 231/240 = 0.9625 (9 incompatible-regex)
const SONARR_MIN_RATE: f64 = 0.948;
const RADARR_MIN_RATE: f64 = 0.962;

#[test]
fn imports_real_trash_sonarr_custom_formats_at_high_rate() {
    let report = report_for("sonarr");
    assert!(report.total >= 200, "expected the full Sonarr CF set");
    assert!(
        report.supported_rate() >= SONARR_MIN_RATE,
        "Sonarr supported-rate {:.4} fell below ratchet {SONARR_MIN_RATE}; skipped: {:?}",
        report.supported_rate(),
        report.unsupported_counts,
    );
    assert_match_context_builds(&report, "sonarr");
}

#[test]
fn imports_real_trash_radarr_custom_formats_at_high_rate() {
    let report = report_for("radarr");
    assert!(report.total >= 200, "expected the full Radarr CF set");
    assert!(
        report.supported_rate() >= RADARR_MIN_RATE,
        "Radarr supported-rate {:.4} fell below ratchet {RADARR_MIN_RATE}; skipped: {:?}",
        report.supported_rate(),
        report.unsupported_counts,
    );
    assert_match_context_builds(&report, "radarr");
}

#[test]
fn skips_are_counted_not_silently_dropped() {
    // Every skipped CF must be accounted for: supported + skipped == total, and
    // the per-implementation tally must sum to the skip count.
    for app in ["sonarr", "radarr"] {
        let report = import_app(app);
        assert_eq!(
            report.supported() + report.skipped.len(),
            report.total,
            "{app}: supported + skipped must equal total (nothing silently dropped)"
        );
        let tallied: usize = report.unsupported_counts.values().sum();
        assert_eq!(
            tallied,
            report.skipped.len(),
            "{app}: per-implementation counts must sum to the skip count"
        );
    }
}
