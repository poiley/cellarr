//! Custom-format **matching** oracle: define one CF set, configure it in a live
//! Sonarr, import the equivalent into cellarr, and diff which CFs each side
//! reports as matching over the corpus titles. This oracles the decision
//! engine's CF condition matching — the part of decisions most likely to diverge
//! (notably regex-dialect differences: .NET vs Rust `regex`/`fancy-regex`).
//!
//! `#[ignore]`; self-skips without `CELLARR_ORACLE_SONARR[_KEY]`. Run via
//! `just oracle-cf` (or manually). Results: `target/parity/cf-*`. See docs/parity/.
//!
//! Scope: ReleaseTitle-regex CFs (the bulk of real TRaSH CFs encode HDR/audio/
//! repack/etc. as title regexes), so this is both representative and the sharpest
//! regex-parity probe.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use cellarr_core::{IndexerId, Protocol, Release};
use cellarr_decide::matching::MatchContext;
use cellarr_decide::trash::import_trash_custom_formats;
use cellarr_parse::parse_title;
use serde::Deserialize;

// (name, regex). Same name both sides so matched-sets are comparable.
const CFS: &[(&str, &str)] = &[
    ("Repack/Proper", r"\b(repack|proper)\b"),
    ("x265/HEVC", r"(x265|h265|hevc)"),
    ("Remux", r"\bremux\b"),
    ("Atmos", r"\batmos\b"),
    ("DV/HDR10", r"\b(dv|dovi|dolby.?vision|hdr10|hdr)\b"),
    ("AMZN", r"\b(amzn|amazon)\b"),
    ("MULTi", r"\bmulti\b"),
    ("10bit", r"(10.?bit)"),
];

#[derive(Deserialize)]
struct CorpusFile {
    #[serde(default)]
    case: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    input: String,
}

fn sonarr_cf_json(name: &str, regex: &str) -> serde_json::Value {
    // Sonarr's POST /customformat shape: fields as an array of {name,value}.
    serde_json::json!({
        "name": name,
        "includeCustomFormatWhenRenaming": false,
        "specifications": [{
            "name": name,
            "implementation": "ReleaseTitleSpecification",
            "negate": false, "required": true,
            "fields": [{"name": "value", "value": regex}]
        }]
    })
}

fn cellarr_trash_json() -> String {
    // cellarr's TRaSH import shape: fields as an object {value}.
    let arr: Vec<serde_json::Value> = CFS
        .iter()
        .map(|(name, regex)| {
            serde_json::json!({
                "trash_id": name,
                "name": name,
                "specifications": [{
                    "name": name,
                    "implementation": "ReleaseTitleSpecification",
                    "negate": false, "required": true,
                    "fields": {"value": regex}
                }]
            })
        })
        .collect();
    serde_json::to_string(&arr).unwrap()
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

#[test]
#[ignore = "CF-matching oracle; run via `just oracle-cf` with a live Sonarr"]
fn oracle_cf_matching() {
    let Ok(sonarr) = std::env::var("CELLARR_ORACLE_SONARR") else {
        eprintln!("CELLARR_ORACLE_SONARR not set; skipping. Use `just oracle-cf`.");
        return;
    };
    let key = std::env::var("CELLARR_ORACLE_SONARR_KEY").unwrap_or_default();
    let client = reqwest::blocking::Client::new();

    // Configure the CFs in Sonarr (idempotent enough for a fresh --tmpfs container;
    // a 400 "already exists" is ignored).
    for (name, regex) in CFS {
        let _ = client
            .post(format!("{sonarr}/api/v3/customformat"))
            .header("X-Api-Key", &key)
            .json(&sonarr_cf_json(name, regex))
            .send();
    }

    // Import the same set into cellarr.
    let scores = std::collections::HashMap::new();
    let formats = import_trash_custom_formats(&cellarr_trash_json(), &scores)
        .expect("cellarr imports the CF set");
    let ctx = MatchContext::new(&formats).expect("match context");

    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/parse")
        .canonicalize()
        .expect("corpus dir");
    let mut files: Vec<PathBuf> = fs::read_dir(&corpus)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();

    let mut titles = 0u64;
    let mut exact = 0u64;
    let mut mismatches: Vec<serde_json::Value> = Vec::new();
    let mut per_cf_disagree: std::collections::BTreeMap<String, u64> = Default::default();

    for path in files {
        let text = fs::read_to_string(&path).unwrap();
        let parsed_file: CorpusFile = toml::from_str(&text).unwrap();
        for case in parsed_file.case {
            let title = case.input;
            // cellarr matched set
            let parsed = parse_title(&title);
            let rel = release_for(&title);
            let cell: BTreeSet<String> = formats
                .iter()
                .filter(|f| ctx.matches(f, &rel, &parsed))
                .map(|f| f.name.clone())
                .collect();
            // Sonarr matched set
            let resp = client
                .get(format!("{sonarr}/api/v3/parse"))
                .query(&[("title", title.as_str())])
                .header("X-Api-Key", &key)
                .send();
            let Ok(resp) = resp else { continue };
            let json: serde_json::Value = resp.json().unwrap_or_default();
            let son: BTreeSet<String> = json
                .get("customFormats")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.get("name").and_then(|n| n.as_str()).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();

            titles += 1;
            if cell == son {
                exact += 1;
            } else {
                for cf in cell.symmetric_difference(&son) {
                    *per_cf_disagree.entry(cf.clone()).or_default() += 1;
                }
                mismatches.push(serde_json::json!({
                    "title": title,
                    "cellarr": cell.iter().collect::<Vec<_>>(),
                    "sonarr": son.iter().collect::<Vec<_>>(),
                }));
            }
        }
    }

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/parity");
    fs::create_dir_all(&out).ok();
    fs::write(
        out.join("cf-mismatches.jsonl"),
        mismatches
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .ok();
    let summary = serde_json::json!({
        "titles": titles, "exact": exact,
        "exact_rate": if titles == 0 { 0.0 } else { exact as f64 / titles as f64 },
        "per_cf_disagreements": per_cf_disagree,
        "mismatch_count": mismatches.len(),
    });
    fs::write(
        out.join("cf-results.json"),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .ok();

    println!("\n=== cellarr CF-matching parity vs Sonarr ===");
    println!(
        "CFs: {}  titles: {titles}  exact-set-match: {exact}  mismatches: {}",
        CFS.len(),
        mismatches.len()
    );
    if !per_cf_disagree.is_empty() {
        println!("per-CF disagreements:");
        for (cf, n) in &per_cf_disagree {
            println!("  {cf:14} {n}");
        }
    }
    println!("results: target/parity/cf-results.json + cf-mismatches.jsonl");
}
