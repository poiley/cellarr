//! Custom-format **scoring** oracle: extends the matching oracle (oracle_cf.rs,
//! already 100%) to *scores*. We assign a representative TRaSH-like score to each
//! CF, configure those CFs **and** a quality profile carrying those scores in a
//! live Sonarr, import the identical CFs+scores into cellarr via
//! `import_trash_custom_formats`, and then compare, per corpus title, the
//! authoritative Sonarr CF-score against cellarr's `cellarr_decide::score`.
//!
//! ## How "Sonarr's score" is obtained — honestly
//! Sonarr's standalone `GET /api/v3/parse` returns the **matched custom-format
//! set** (its own matcher, .NET regex) but reports `customFormatScore: 0` for a
//! bare parse — that field is only populated when scoring runs against a *series'*
//! quality profile during a real decision, which `parse` does not do (no
//! series/profile context; passing `qualityProfileId` is ignored). This is the
//! known black-box limitation recorded in docs/parity/decision-gaps.md.
//!
//! So we reconstruct Sonarr's authoritative score with Sonarr's *own* formula
//! (`CustomFormatCalculationService`: the score of a release = the sum of the
//! profile `formatItems[]` scores of every custom format that matched):
//!   sonarr_score(title) = Σ over (CFs that Sonarr's /api/v3/parse matched)
//!                           of (that CF's score in the profile we PUT + read back).
//! Both inputs are Sonarr's own: the matched set comes from Sonarr's matcher, the
//! per-CF scores come from the live profile's `formatItems` (PUT then GET-verified).
//! cellarr's number comes entirely from cellarr's engine. A divergence therefore
//! means cellarr matched a different set OR summed differently — exactly what we
//! want to catch.
//!
//! `#[ignore]`; self-skips without `CELLARR_ORACLE_SONARR[_KEY]`. Run via
//! `just oracle-cf-score`. Results: `target/parity/cf-score-*`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;

use cellarr_core::{IndexerId, Protocol, Release};
use cellarr_decide::matching::MatchContext;
use cellarr_decide::scoring::score;
use cellarr_decide::trash::import_trash_custom_formats;
use cellarr_parse::parse_title;
use serde::Deserialize;

/// (name, regex, score). Representative TRaSH-like CF scores: x265/HEVC is a
/// hard-negative guard (TRaSH uses -10000 to veto HEVC where unwanted), Remux a
/// large positive, HDR/DV a tier bump, Atmos/AMZN/10bit small bumps, MULTi a
/// small penalty, Repack/Proper the standard +5.
const CFS: &[(&str, &str, i32)] = &[
    ("Repack/Proper", r"\b(repack|proper)\b", 5),
    ("x265/HEVC", r"(x265|h265|hevc)", -10000),
    ("Remux", r"\bremux\b", 1000),
    ("Atmos", r"\batmos\b", 200),
    ("DV/HDR10", r"\b(dv|dovi|dolby.?vision|hdr10|hdr)\b", 500),
    ("AMZN", r"\b(amzn|amazon)\b", 100),
    ("MULTi", r"\bmulti\b", -50),
    ("10bit", r"(10.?bit)", 50),
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
    let arr: Vec<serde_json::Value> = CFS
        .iter()
        .map(|(name, regex, _)| {
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
#[ignore = "CF-score oracle; run via `just oracle-cf-score` with a live Sonarr"]
fn oracle_cf_score() {
    let Ok(sonarr) = std::env::var("CELLARR_ORACLE_SONARR") else {
        eprintln!("CELLARR_ORACLE_SONARR not set; skipping. Use `just oracle-cf-score`.");
        return;
    };
    let key = std::env::var("CELLARR_ORACLE_SONARR_KEY").unwrap_or_default();
    let client = reqwest::blocking::Client::new();
    let api = format!("{sonarr}/api/v3");

    // 1) Configure the CFs in Sonarr (idempotent for a fresh --tmpfs container).
    for (name, regex, _) in CFS {
        let _ = client
            .post(format!("{api}/customformat"))
            .header("X-Api-Key", &key)
            .json(&sonarr_cf_json(name, regex))
            .send();
    }

    // 2) PUT our scores into a quality profile's formatItems, then GET it back so
    //    the score table we compare against is Sonarr's own stored value.
    //    HD-1080p is profile id 4 on a default Sonarr; fall back to the first
    //    profile if not present.
    let profiles: serde_json::Value = client
        .get(format!("{api}/qualityprofile"))
        .header("X-Api-Key", &key)
        .send()
        .expect("list qualityprofile")
        .json()
        .expect("qualityprofile json");
    let profile_id = profiles
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|p| p.get("id").and_then(|v| v.as_i64()) == Some(4))
                .or_else(|| a.first())
        })
        .and_then(|p| p.get("id").and_then(|v| v.as_i64()))
        .expect("a quality profile id");

    let mut profile: serde_json::Value = client
        .get(format!("{api}/qualityprofile/{profile_id}"))
        .header("X-Api-Key", &key)
        .send()
        .expect("get profile")
        .json()
        .expect("profile json");

    // desired score table by CF name
    let want_score: HashMap<&str, i32> = CFS.iter().map(|(n, _, s)| (*n, *s)).collect();
    if let Some(items) = profile
        .get_mut("formatItems")
        .and_then(|v| v.as_array_mut())
    {
        for item in items.iter_mut() {
            if let Some(n) = item.get("name").and_then(|v| v.as_str()) {
                if let Some(s) = want_score.get(n) {
                    item["score"] = serde_json::json!(*s);
                }
            }
        }
    }
    let put = client
        .put(format!("{api}/qualityprofile/{profile_id}"))
        .header("X-Api-Key", &key)
        .json(&profile)
        .send();
    assert!(
        put.as_ref()
            .map(|r| r.status().is_success())
            .unwrap_or(false),
        "PUT qualityprofile must succeed: {put:?}"
    );

    // Read it back: this map (CF name -> score) is Sonarr's authoritative table.
    let profile_back: serde_json::Value = client
        .get(format!("{api}/qualityprofile/{profile_id}"))
        .header("X-Api-Key", &key)
        .send()
        .expect("get profile back")
        .json()
        .expect("profile json");
    let mut sonarr_scores: HashMap<String, i32> = HashMap::new();
    if let Some(items) = profile_back.get("formatItems").and_then(|v| v.as_array()) {
        for item in items {
            if let (Some(n), Some(s)) = (
                item.get("name").and_then(|v| v.as_str()),
                item.get("score").and_then(|v| v.as_i64()),
            ) {
                sonarr_scores.insert(n.to_string(), s as i32);
            }
        }
    }
    // Sanity: every CF we configured must be present with the score we set.
    for (name, _, s) in CFS {
        assert_eq!(
            sonarr_scores.get(*name).copied(),
            Some(*s),
            "Sonarr profile must carry our score for {name}"
        );
    }

    // 3) Import the SAME CFs + scores into cellarr.
    let scores_map: HashMap<String, i32> =
        CFS.iter().map(|(n, _, s)| (n.to_string(), *s)).collect();
    let formats = import_trash_custom_formats(&cellarr_trash_json(), &scores_map)
        .expect("cellarr imports the CF set with scores");
    let ctx = MatchContext::new(&formats).expect("match context");

    // 4) Walk the corpus and compare per-title scores.
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
    let mut per_cf_disagree: BTreeMap<String, u64> = Default::default();

    for path in files {
        let text = fs::read_to_string(&path).unwrap();
        let parsed_file: CorpusFile = toml::from_str(&text).unwrap();
        for case in parsed_file.case {
            let title = case.input;

            // cellarr score
            let parsed = parse_title(&title);
            let rel = release_for(&title);
            let cell_score = score(&rel, &parsed, &formats, &ctx);
            let cell_matched: BTreeSet<String> = formats
                .iter()
                .filter(|f| ctx.matches(f, &rel, &parsed))
                .map(|f| f.name.clone())
                .collect();

            // Sonarr score = Σ(profile score of each CF that Sonarr's parse matched)
            let resp = client
                .get(format!("{api}/parse"))
                .query(&[("title", title.as_str())])
                .header("X-Api-Key", &key)
                .send();
            let Ok(resp) = resp else { continue };
            let json: serde_json::Value = resp.json().unwrap_or_default();
            let son_matched: BTreeSet<String> = json
                .get("customFormats")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.get("name").and_then(|n| n.as_str()).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let son_score: i32 = son_matched
                .iter()
                .map(|n| sonarr_scores.get(n).copied().unwrap_or(0))
                .fold(0i32, i32::saturating_add);

            titles += 1;
            if cell_score == son_score {
                exact += 1;
            } else {
                for cf in cell_matched.symmetric_difference(&son_matched) {
                    *per_cf_disagree.entry(cf.clone()).or_default() += 1;
                }
                mismatches.push(serde_json::json!({
                    "title": title,
                    "cellarr_score": cell_score,
                    "sonarr_score": son_score,
                    "cellarr_matched": cell_matched.iter().collect::<Vec<_>>(),
                    "sonarr_matched": son_matched.iter().collect::<Vec<_>>(),
                }));
            }
        }
    }

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/parity");
    fs::create_dir_all(&out).ok();
    fs::write(
        out.join("cf-score-mismatches.jsonl"),
        mismatches
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .ok();
    let summary = serde_json::json!({
        "titles": titles,
        "exact": exact,
        "exact_rate": if titles == 0 { 0.0 } else { exact as f64 / titles as f64 },
        "per_cf_disagreements": per_cf_disagree,
        "mismatch_count": mismatches.len(),
        "scores": scores_map,
    });
    fs::write(
        out.join("cf-score-results.json"),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .ok();

    println!("\n=== cellarr CF-SCORE parity vs Sonarr ===");
    println!(
        "CFs: {}  titles: {titles}  exact-score-match: {exact}  mismatches: {}",
        CFS.len(),
        mismatches.len()
    );
    if !per_cf_disagree.is_empty() {
        println!("per-CF disagreements (cause of score gaps):");
        for (cf, n) in &per_cf_disagree {
            println!("  {cf:14} {n}");
        }
    }
    println!("results: target/parity/cf-score-results.json + cf-score-mismatches.jsonl");

    // The whole point: with matching already at 100% and identical score tables,
    // scores must match exactly.
    assert_eq!(
        mismatches.len(),
        0,
        "CF-score parity must be exact; see target/parity/cf-score-mismatches.jsonl"
    );
}
