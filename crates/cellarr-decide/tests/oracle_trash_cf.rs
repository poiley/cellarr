//! Full-corpus, **real-TRaSH-set** custom-format MATCHING + SCORE oracle.
//!
//! This is the heavy sibling of `oracle_cf.rs` / `oracle_cf_score.rs` (which use a
//! tiny hand-written 8-CF set). Here we drive the *entire* curated TRaSH-Guides CF
//! set (the fixtures under `tests/fixtures/trash/{sonarr,radarr}/cf/*.json`) into a
//! live Sonarr **and** Radarr, import the identical set into cellarr, and diff —
//! per corpus title, routed by path to the right app — both:
//!   * the matched-CF set (cellarr's matcher vs the app's `/api/v3/parse`
//!     `customFormats[]`), as exact-set equality, and
//!   * the CF score (cellarr's `cellarr_decide::score` vs Σ of the app profile's
//!     `formatItems[]` score for each CF the app matched).
//!
//! ## How "the app's score" is obtained
//! As documented in docs/parity/decision-gaps.md, `/api/v3/parse` returns the
//! matched CF *set* (the app's own .NET-regex matcher) but reports
//! `customFormatScore: 0` for a bare parse. So we reconstruct the authoritative
//! score with the app's own formula: score(title) = Σ over (CFs the app matched)
//! of (that CF's score in the quality profile we PUT and read back). Both inputs
//! are the app's: the matched set from its matcher, the per-CF scores from the
//! live profile.
//!
//! ## Routing
//! Sonarr CFs + Sonarr-shaped corpus titles go to Sonarr; Radarr CFs + movie
//! titles go to Radarr. `corpus/parse/movie_*` and `corpus/upstream/radarr/*` are
//! Radarr; everything else (episode/season/anime/daily/etc.) is Sonarr.
//!
//! ## Divergence tagging
//! Every mismatch is tagged with a heuristic class so the long tail is
//! catalogued, not chased: `regex-dialect`, `app-builtin-cf`, `unsupported-spec`,
//! `cellarr-stronger`, or `unclassified`. See docs/parity/decision-gaps.md.
//!
//! `#[ignore]`; self-skips without the env vars. Run via `just oracle-trash-cf`.
//! Results: `target/parity/trash-cf-*`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;

use cellarr_core::{CustomFormat, IndexerId, Protocol, Release};
use cellarr_decide::matching::MatchContext;
use cellarr_decide::scoring::score;
use cellarr_decide::trash::{
    import_trash_custom_formats_counted_for_app, TrashApp, TrashImportReport,
};
use cellarr_parse::parse_title;
use serde::Deserialize;

/// Bounded concurrency for the per-title `/api/v3/parse` probe.
const WORKERS: usize = 8;
/// Per-call timeout (the task bounds every external wait at <=10s).
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Deserialize)]
struct CorpusFile {
    #[serde(default)]
    case: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    input: String,
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/trash")
}

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .canonicalize()
        .expect("corpus dir")
}

/// Load every CF JSON file under `cf_dir` as one JSON array (TRaSH ships one CF
/// per file). Returns the raw `serde_json::Value`s so we can both translate them
/// to the Servarr POST shape and re-serialize for cellarr's importer.
fn load_cf_array(cf_dir: &Path) -> Vec<serde_json::Value> {
    let mut files: Vec<PathBuf> = fs::read_dir(cf_dir)
        .unwrap_or_else(|e| panic!("read {cf_dir:?}: {e}"))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|p| {
            let text = fs::read_to_string(p).unwrap_or_else(|e| panic!("read {p:?}: {e}"));
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {p:?}: {e}"))
        })
        .collect()
}

fn load_scores(path: &Path) -> HashMap<String, i32> {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path:?}: {e}"))
}

/// Translate a TRaSH/Servarr-export CF (`fields` is an object `{value|min|max}`)
/// into the shape `POST /api/v3/customformat` accepts (`fields` is an array of
/// `{name, value}`). Only `value`/`min`/`max` carry meaning for our spec kinds.
fn to_post_shape(cf: &serde_json::Value) -> serde_json::Value {
    let specs = cf
        .get("specifications")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    let specs: Vec<serde_json::Value> = specs
        .iter()
        .map(|spec| {
            let mut fields = Vec::new();
            if let Some(obj) = spec.get("fields").and_then(|f| f.as_object()) {
                for (k, v) in obj {
                    fields.push(serde_json::json!({ "name": k, "value": v }));
                }
            }
            serde_json::json!({
                "name": spec.get("name").cloned().unwrap_or(serde_json::Value::Null),
                "implementation": spec.get("implementation").cloned().unwrap_or(serde_json::Value::Null),
                "negate": spec.get("negate").and_then(|x| x.as_bool()).unwrap_or(false),
                "required": spec.get("required").and_then(|x| x.as_bool()).unwrap_or(false),
                "fields": fields,
            })
        })
        .collect();
    serde_json::json!({
        "name": cf.get("name").cloned().unwrap_or(serde_json::Value::Null),
        "includeCustomFormatWhenRenaming": false,
        "specifications": specs,
    })
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

/// The corpus files to feed each app, routed by path. Returns absolute paths.
fn corpus_titles(app: App) -> Vec<String> {
    let root = corpus_root();
    let mut paths: Vec<PathBuf> = Vec::new();
    match app {
        App::Sonarr => {
            // Shared parse corpus, excluding movie-only files.
            for p in glob_toml(&root.join("parse")) {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                if !name.starts_with("movie_") {
                    paths.push(p);
                }
            }
            paths.extend(glob_toml(&root.join("upstream/sonarr")));
            paths.extend(glob_toml(&root.join("anime")));
        }
        App::Radarr => {
            for p in glob_toml(&root.join("parse")) {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                // Movie files + neutral quality/group/title vectors exercise movie CFs.
                if name.starts_with("movie_")
                    || name == "quality.toml"
                    || name == "release_group.toml"
                {
                    paths.push(p);
                }
            }
            paths.extend(glob_toml(&root.join("upstream/radarr")));
        }
    }
    paths.sort();
    paths.dedup();
    let mut titles: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();
    for path in paths {
        let text = fs::read_to_string(&path).unwrap();
        let file: CorpusFile = toml::from_str(&text).unwrap_or(CorpusFile { case: vec![] });
        for case in file.case {
            if seen.insert(case.input.clone()) {
                titles.push(case.input);
            }
        }
    }
    titles
}

fn glob_toml(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum App {
    Sonarr,
    Radarr,
}

impl App {
    fn name(self) -> &'static str {
        match self {
            App::Sonarr => "sonarr",
            App::Radarr => "radarr",
        }
    }
    fn env_base(self) -> &'static str {
        match self {
            App::Sonarr => "CELLARR_ORACLE_SONARR",
            App::Radarr => "CELLARR_ORACLE_RADARR",
        }
    }
    fn env_key(self) -> &'static str {
        match self {
            App::Sonarr => "CELLARR_ORACLE_SONARR_KEY",
            App::Radarr => "CELLARR_ORACLE_RADARR_KEY",
        }
    }
}

/// Classify a per-title divergence into a catalogued class (best-effort heuristic).
///
/// * `cellarr-stronger`  — cellarr matched a CF the app did not, and cellarr's set
///   is a strict superset (we are more permissive; usually a regex-dialect or a
///   spec the app gates differently).
/// * `app-builtin-cf`    — the app matched a CF name cellarr never imported (the
///   app carries a built-in/conditioned CF cellarr's set lacks), i.e. an
///   app-only name in the diff.
/// * `unsupported-spec`  — the disagreeing CF was skipped by cellarr's importer
///   (an unsupported `implementation`), so cellarr can never match it.
/// * `regex-dialect`     — the disagreeing CF is a single-ReleaseTitle CF (the
///   classic .NET-vs-fancy-regex axis).
/// * `unclassified`      — none of the above.
fn classify(
    cellarr: &BTreeSet<String>,
    app: &BTreeSet<String>,
    imported_names: &BTreeSet<String>,
    skipped_names: &BTreeSet<String>,
    title_only_cfs: &BTreeSet<String>,
) -> String {
    let only_cellarr: Vec<&String> = cellarr.difference(app).collect();
    let only_app: Vec<&String> = app.difference(cellarr).collect();

    // An app-matched CF that cellarr never imported at all.
    if only_app.iter().any(|n| !imported_names.contains(*n)) {
        if only_app.iter().any(|n| skipped_names.contains(*n)) {
            return "unsupported-spec".to_string();
        }
        return "app-builtin-cf".to_string();
    }
    if only_cellarr.is_empty() && !only_app.is_empty() {
        // App matched something cellarr imported but did not match.
        if only_app.iter().all(|n| title_only_cfs.contains(*n)) {
            return "regex-dialect".to_string();
        }
        return "unclassified".to_string();
    }
    if only_app.is_empty() && !only_cellarr.is_empty() {
        return "cellarr-stronger".to_string();
    }
    // Both sides have exclusive entries — mixed; lean on regex-dialect when both
    // diffs are title-only CFs (the dominant cause), else unclassified.
    if only_cellarr
        .iter()
        .chain(only_app.iter())
        .all(|n| title_only_cfs.contains(*n))
    {
        "regex-dialect".to_string()
    } else {
        "unclassified".to_string()
    }
}

/// CFs whose conditions are *only* ReleaseTitle regexes — the set most exposed to
/// regex-dialect divergence.
fn title_only_cf_names(formats: &[CustomFormat]) -> BTreeSet<String> {
    formats
        .iter()
        .filter(|f| {
            !f.conditions.is_empty()
                && f.conditions
                    .iter()
                    .all(|c| matches!(c.kind, cellarr_core::ConditionKind::ReleaseTitle { .. }))
        })
        .map(|f| f.name.clone())
        .collect()
}

struct AppResult {
    app: &'static str,
    cf_total: usize,
    cf_posted: usize,
    cf_4xx: usize,
    cellarr_imported: usize,
    cellarr_skipped: usize,
    titles: u64,
    match_exact: u64,
    /// Titles where the matched sets agree once CFs cellarr can never model
    /// (skipped/unsupported `implementation`s — e.g. `ReleaseTypeSpecification`)
    /// are excluded from the app's set. This isolates the *CF-matching-algebra*
    /// parity from the genuinely-unsupported long tail.
    modelable_match_exact: u64,
    score_exact: u64,
    match_mismatches: Vec<serde_json::Value>,
    score_mismatches: Vec<serde_json::Value>,
    class_tally: BTreeMap<String, u64>,
    per_cf_disagree: BTreeMap<String, u64>,
}

fn run_app(app: App) -> Option<AppResult> {
    let Ok(base) = std::env::var(app.env_base()) else {
        eprintln!("{} not set; skipping {}.", app.env_base(), app.name());
        return None;
    };
    let key = std::env::var(app.env_key()).unwrap_or_default();
    let api = format!("{base}/api/v3");
    let client = reqwest::blocking::Client::builder()
        .timeout(CALL_TIMEOUT)
        .build()
        .expect("client");

    // --- 1) POST the full CF set into the app, tolerating + counting 4xx. -------
    let fx = fixtures_dir().join(app.name());
    let raw_cfs = load_cf_array(&fx.join("cf"));
    let scores = load_scores(&fx.join("scores.default.json"));
    let cf_total = raw_cfs.len();
    let mut cf_posted = 0usize;
    let mut cf_4xx = 0usize;
    for cf in &raw_cfs {
        let body = to_post_shape(cf);
        match client
            .post(format!("{api}/customformat"))
            .header("X-Api-Key", &key)
            .json(&body)
            .send()
        {
            Ok(resp) => {
                if resp.status().is_success() {
                    cf_posted += 1;
                } else if resp.status().is_client_error() {
                    cf_4xx += 1;
                }
            }
            Err(_) => cf_4xx += 1,
        }
    }

    // --- 2) Build the authoritative app score table via a profile's formatItems.
    // POSTing CFs auto-adds a (score 0) formatItem per CF to every profile. GET a
    // profile, set scores by CF name from the TRaSH default map, PUT, GET back.
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

    // Map CF name -> trash default score (via name->trash_id->score).
    let name_to_id: HashMap<String, String> = raw_cfs
        .iter()
        .filter_map(|cf| {
            let n = cf.get("name").and_then(|v| v.as_str())?.to_string();
            let id = cf.get("trash_id").and_then(|v| v.as_str())?.to_string();
            Some((n, id))
        })
        .collect();
    let want_score = |cf_name: &str| -> i32 {
        name_to_id
            .get(cf_name)
            .and_then(|id| scores.get(id).copied())
            .unwrap_or(0)
    };
    if let Some(items) = profile
        .get_mut("formatItems")
        .and_then(|v| v.as_array_mut())
    {
        for item in items.iter_mut() {
            if let Some(n) = item
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::to_string)
            {
                item["score"] = serde_json::json!(want_score(&n));
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
        "{}: PUT qualityprofile must succeed: {put:?}",
        app.name()
    );
    let profile_back: serde_json::Value = client
        .get(format!("{api}/qualityprofile/{profile_id}"))
        .header("X-Api-Key", &key)
        .send()
        .expect("get profile back")
        .json()
        .expect("profile json");
    let mut app_scores: HashMap<String, i32> = HashMap::new();
    if let Some(items) = profile_back.get("formatItems").and_then(|v| v.as_array()) {
        for item in items {
            if let (Some(n), Some(s)) = (
                item.get("name").and_then(|v| v.as_str()),
                item.get("score").and_then(|v| v.as_i64()),
            ) {
                app_scores.insert(n.to_string(), s as i32);
            }
        }
    }

    // --- 3) Import the identical set into cellarr. -----------------------------
    let cellarr_json = serde_json::to_string(&raw_cfs).expect("re-serialize CFs");
    let trash_app = match app {
        App::Sonarr => TrashApp::Sonarr,
        App::Radarr => TrashApp::Radarr,
    };
    let report: TrashImportReport =
        import_trash_custom_formats_counted_for_app(&cellarr_json, &scores, trash_app)
            .expect("cellarr import");
    let formats = report.formats.clone();
    let ctx = MatchContext::new(&formats).expect("match context");
    let imported_names: BTreeSet<String> = formats.iter().map(|f| f.name.clone()).collect();
    let skipped_names: BTreeSet<String> = report.skipped.iter().map(|(n, _)| n.clone()).collect();
    let title_only = title_only_cf_names(&formats);

    // --- 4) Walk the routed corpus, bounded-concurrent app probes. -------------
    let titles = corpus_titles(app);
    let app_matched = probe_app_matched(&base, &key, &titles);

    let mut res = AppResult {
        app: app.name(),
        cf_total,
        cf_posted,
        cf_4xx,
        cellarr_imported: report.supported(),
        cellarr_skipped: report.skipped.len(),
        titles: 0,
        match_exact: 0,
        modelable_match_exact: 0,
        score_exact: 0,
        match_mismatches: Vec::new(),
        score_mismatches: Vec::new(),
        class_tally: BTreeMap::new(),
        per_cf_disagree: BTreeMap::new(),
    };

    for title in &titles {
        let Some(son) = app_matched.get(title) else {
            continue; // app probe failed for this title; skip (don't count)
        };
        let parsed = parse_title(title);
        let rel = release_for(title);
        let cell: BTreeSet<String> = formats
            .iter()
            .filter(|f| ctx.matches(f, &rel, &parsed))
            .map(|f| f.name.clone())
            .collect();
        let cell_score = score(&rel, &parsed, &formats, &ctx);
        let son_score: i32 = son
            .iter()
            .map(|n| app_scores.get(n).copied().unwrap_or(0))
            .fold(0i32, i32::saturating_add);

        res.titles += 1;
        let matched_equal = cell == *son;
        if matched_equal {
            res.match_exact += 1;
        }
        // Modelable parity: drop from the app's set every CF cellarr could never
        // model (it was skipped on import, or the name was never imported at all
        // — both mean an unsupported `implementation` cellarr cannot match), then
        // compare. This isolates the CF-matching algebra from the unsupported tail.
        let son_modelable: BTreeSet<String> = son
            .iter()
            .filter(|n| imported_names.contains(*n) && !skipped_names.contains(*n))
            .cloned()
            .collect();
        if cell == son_modelable {
            res.modelable_match_exact += 1;
        }
        if !matched_equal {
            for cf in cell.symmetric_difference(son) {
                *res.per_cf_disagree.entry(cf.clone()).or_default() += 1;
            }
            let class = classify(&cell, son, &imported_names, &skipped_names, &title_only);
            *res.class_tally.entry(class.clone()).or_default() += 1;
            res.match_mismatches.push(serde_json::json!({
                "title": title,
                "class": class,
                "cellarr": cell.iter().collect::<Vec<_>>(),
                "app": son.iter().collect::<Vec<_>>(),
            }));
        }
        if cell_score == son_score {
            res.score_exact += 1;
        } else {
            res.score_mismatches.push(serde_json::json!({
                "title": title,
                "class": if matched_equal { "score-only".to_string() }
                         else { classify(&cell, son, &imported_names, &skipped_names, &title_only) },
                "cellarr_score": cell_score,
                "app_score": son_score,
                "cellarr": cell.iter().collect::<Vec<_>>(),
                "app": son.iter().collect::<Vec<_>>(),
            }));
        }
    }
    Some(res)
}

/// Probe `GET /api/v3/parse` for every title with bounded concurrency, returning
/// title -> matched-CF-name set. A failed/absent probe is simply omitted.
fn probe_app_matched(
    base: &str,
    key: &str,
    titles: &[String],
) -> HashMap<String, BTreeSet<String>> {
    let api = Arc::new(format!("{base}/api/v3"));
    let key = Arc::new(key.to_string());
    let (tx, rx) = mpsc::channel::<(String, Option<BTreeSet<String>>)>();
    let chunks: Vec<Vec<String>> = {
        let mut v: Vec<Vec<String>> = (0..WORKERS).map(|_| Vec::new()).collect();
        for (i, t) in titles.iter().enumerate() {
            v[i % WORKERS].push(t.clone());
        }
        v
    };
    std::thread::scope(|scope| {
        for chunk in chunks {
            let tx = tx.clone();
            let api = Arc::clone(&api);
            let key = Arc::clone(&key);
            scope.spawn(move || {
                let client = reqwest::blocking::Client::builder()
                    .timeout(CALL_TIMEOUT)
                    .build()
                    .expect("client");
                for title in chunk {
                    let set = client
                        .get(format!("{api}/parse"))
                        .query(&[("title", title.as_str())])
                        .header("X-Api-Key", key.as_str())
                        .send()
                        .ok()
                        .and_then(|r| r.json::<serde_json::Value>().ok())
                        .map(|json| {
                            json.get("customFormats")
                                .and_then(|x| x.as_array())
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|c| {
                                            c.get("name")
                                                .and_then(|n| n.as_str())
                                                .map(str::to_string)
                                        })
                                        .collect::<BTreeSet<String>>()
                                })
                                .unwrap_or_default()
                        });
                    let _ = tx.send((title, set));
                }
            });
        }
        drop(tx);
        let mut out = HashMap::new();
        for (title, set) in rx {
            if let Some(set) = set {
                out.insert(title, set);
            }
        }
        out
    })
}

fn write_results(res: &AppResult) {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/parity");
    fs::create_dir_all(&out).ok();
    let app = res.app;
    let rate = |n: u64| {
        if res.titles == 0 {
            0.0
        } else {
            n as f64 / res.titles as f64
        }
    };
    let match_rate = rate(res.match_exact);
    let modelable_rate = rate(res.modelable_match_exact);
    let score_rate = rate(res.score_exact);
    let summary = serde_json::json!({
        "app": app,
        "cf_total": res.cf_total,
        "cf_posted_ok": res.cf_posted,
        "cf_4xx": res.cf_4xx,
        "cellarr_imported": res.cellarr_imported,
        "cellarr_skipped": res.cellarr_skipped,
        "titles": res.titles,
        "match_exact": res.match_exact,
        "cf_match_parity": match_rate,
        "modelable_match_exact": res.modelable_match_exact,
        "cf_modelable_match_parity": modelable_rate,
        "score_exact": res.score_exact,
        "cf_score_parity": score_rate,
        "match_mismatch_count": res.match_mismatches.len(),
        "score_mismatch_count": res.score_mismatches.len(),
        "divergence_classes": res.class_tally,
        "per_cf_disagreements": res.per_cf_disagree,
    });
    fs::write(
        out.join(format!("trash-cf-results-{app}.json")),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .ok();
    fs::write(
        out.join(format!("trash-cf-match-mismatches-{app}.jsonl")),
        res.match_mismatches
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .ok();
    fs::write(
        out.join(format!("trash-cf-score-mismatches-{app}.jsonl")),
        res.score_mismatches
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .ok();

    println!("\n=== TRaSH full-set CF parity vs {app} ===");
    println!(
        "  CFs: total={} posted_ok={} 4xx={} | cellarr imported={} skipped={}",
        res.cf_total, res.cf_posted, res.cf_4xx, res.cellarr_imported, res.cellarr_skipped
    );
    println!(
        "  titles={}  match-parity={:.4} ({}/{})  modelable-match-parity={:.4} ({}/{})  score-parity={:.4} ({}/{})",
        res.titles,
        match_rate, res.match_exact, res.titles,
        modelable_rate, res.modelable_match_exact, res.titles,
        score_rate, res.score_exact, res.titles
    );
    if !res.class_tally.is_empty() {
        println!("  divergence classes:");
        for (c, n) in &res.class_tally {
            println!("    {c:18} {n}");
        }
    }
    if !res.per_cf_disagree.is_empty() {
        let mut top: Vec<_> = res.per_cf_disagree.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1));
        println!("  top per-CF disagreements:");
        for (cf, n) in top.into_iter().take(15) {
            println!("    {cf:32} {n}");
        }
    }
    println!("  results: target/parity/trash-cf-results-{app}.json (+ *-mismatches-{app}.jsonl)");
}

/// Ratchet floors, achieved at the pinned fixture commit against the linuxserver
/// Sonarr/Radarr images. `modelable` parity excludes CFs cellarr can never model
/// (unsupported `implementation`s such as `ReleaseTypeSpecification`), isolating
/// the CF-matching-algebra correctness; raw `score` parity is the bottom-line
/// number. Both only move up as parser/spec coverage improves; a drop means a
/// real regression in the matching algebra or import. The remaining tail is
/// catalogued in docs/parity/decision-gaps.md (language-default, ReleaseType,
/// parser-coverage), not chased.
const SONARR_MODELABLE_MIN: f64 = 0.40;
const RADARR_MODELABLE_MIN: f64 = 0.40;
const SONARR_SCORE_MIN: f64 = 0.50;
const RADARR_SCORE_MIN: f64 = 0.35;

#[test]
#[ignore = "full-corpus TRaSH CF oracle; run via `just oracle-trash-cf` with live Sonarr+Radarr"]
fn oracle_trash_cf_full_corpus() {
    let mut ran_any = false;
    for app in [App::Sonarr, App::Radarr] {
        if let Some(res) = run_app(app) {
            ran_any = true;
            write_results(&res);
            let titles = res.titles.max(1) as f64;
            let modelable = res.modelable_match_exact as f64 / titles;
            let score_p = res.score_exact as f64 / titles;
            let (mfloor, sfloor) = match res.app {
                "sonarr" => (SONARR_MODELABLE_MIN, SONARR_SCORE_MIN),
                _ => (RADARR_MODELABLE_MIN, RADARR_SCORE_MIN),
            };
            assert!(
                res.titles > 0,
                "{}: no titles compared — app probes all failed?",
                res.app
            );
            assert!(
                modelable >= mfloor,
                "{}: modelable CF-match parity {modelable:.4} fell below ratchet {mfloor}; \
                 see target/parity/trash-cf-match-mismatches-{}.jsonl",
                res.app,
                res.app,
            );
            assert!(
                score_p >= sfloor,
                "{}: CF-score parity {score_p:.4} fell below ratchet {sfloor}; \
                 see target/parity/trash-cf-score-mismatches-{}.jsonl",
                res.app,
                res.app,
            );
        }
    }
    if !ran_any {
        eprintln!("No oracle apps configured; set CELLARR_ORACLE_SONARR / _RADARR. Skipping.");
    }
}
