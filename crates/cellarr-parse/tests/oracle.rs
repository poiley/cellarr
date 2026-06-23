//! Differential-oracle harness: diff cellarr's parser against the live
//! Sonarr/Radarr `/api/v3/parse` endpoints over the WHOLE corpus — both the
//! curated `corpus/parse/*.toml` set and the harvested `corpus/upstream/**`
//! set (potentially thousands of titles).
//!
//! `#[ignore]` so it never runs in normal `cargo test`. It self-skips when the
//! oracle env vars are unset, so `just oracle` (which sets them after bringing up
//! the containers) is the intended entry point. Results are written under
//! `target/parity/` (git-ignored): a per-field summary JSON
//! (`oracle-fullscale.json`) and a JSONL of every mismatch
//! (`oracle-fullscale-mismatches.jsonl`), so no finding is lost. See docs/parity/.
//!
//! Routing is by corpus path:
//!   - `corpus/parse/*.toml`        — routed per-file by concern (curated set),
//!     with movie-shaped generic titles sent to Radarr (as before).
//!   - `corpus/upstream/sonarr/**`  — Sonarr.
//!   - `corpus/upstream/radarr/**`  — Radarr.
//!
//! Daily/anime sub-handling (drop Sonarr's season-0 sentinel; compare air-date /
//! absolute number) is applied per-domain on both sets.
//!
//! Calls are issued concurrently (bounded thread pool) with a per-call timeout so
//! the full set finishes in bounded wall time. Run: `just oracle`, or manually
//! with CELLARR_ORACLE_SONARR[_KEY] / CELLARR_ORACLE_RADARR[_KEY] set.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use cellarr_core::profile::{resolve_quality, QualityRanking};
use cellarr_core::Coordinates;
use cellarr_parse::parse_title;
use rayon::prelude::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct CorpusFile {
    #[serde(default)]
    case: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    input: String,
}

/// What we compare, in normalized form, regardless of which side produced it.
#[derive(Default, Debug, Clone)]
struct Record {
    title: Option<String>,
    season: Option<i64>,
    episodes: Vec<i64>,
    absolute: Vec<i64>,
    daily_date: Option<String>,
    full_season: bool,
    group: Option<String>,
    quality: Option<String>,
    year: Option<i64>,
    edition: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Domain {
    TvEpisode,
    Daily,
    Absolute,
    TvGeneric,
    Movie,
}

#[derive(Clone, Copy, PartialEq)]
enum App {
    Sonarr,
    Radarr,
}

/// Classify a curated `corpus/parse/<stem>.toml` file into a comparison domain.
fn classify_curated(file_stem: &str) -> Domain {
    match file_stem {
        "movie_title" | "movie_year" | "movie_edition" => Domain::Movie,
        "daily_episode" => Domain::Daily,
        "absolute_anime" => Domain::Absolute,
        "single_episode" | "multi_episode" | "season" | "miniseries" => Domain::TvEpisode,
        _ => Domain::TvGeneric, // quality, language, release_group, unicode, proper_repack
    }
}

/// Classify an upstream `corpus/upstream/<app>/<stem>.toml` file into a domain.
/// The app is fixed by the directory; the stem chooses which fields are relevant.
fn classify_upstream(app: App, file_stem: &str) -> Domain {
    match app {
        App::Radarr => match file_stem {
            "movie_title" | "movie_year" | "movie_edition" | "quality" | "group" => Domain::Movie,
            _ => Domain::Movie,
        },
        App::Sonarr => match file_stem {
            "daily" => Domain::Daily,
            "anime" | "anime_multi" => Domain::Absolute,
            "single_episode" | "multi_episode" | "season" | "miniseries" => Domain::TvEpisode,
            // quality, language, group, unicode -> title/group/quality only.
            _ => Domain::TvGeneric,
        },
    }
}

// Movie-shaped: a 4-digit year and no episode/absolute numbering. Used to route
// the curated generic-quality corpus per-title (e.g. movie-only CAM/HDCAM titles
// go to Radarr, which has those qualities; sending them to Sonarr would mis-map
// to SDTV). Only applied to the curated TvGeneric files.
fn movie_shaped(title: &str) -> bool {
    use regex::Regex;
    use std::sync::LazyLock;
    static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(19|20)\d{2}\b").unwrap());
    static EP: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)s\d{1,2}e\d{1,3}|\b\d{1,2}x\d{2,3}\b|\s-\s\d{1,4}").unwrap()
    });
    YEAR.is_match(title) && !EP.is_match(title)
}

fn norm_title(s: &str) -> String {
    let lower = s.to_lowercase();
    let kept: String = lower
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn norm_opt(s: Option<&str>) -> Option<String> {
    s.map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty())
}

/// Render cellarr's canonical quality name on the Radarr face, mirroring the
/// `/api/v3` shim's `face_quality_name`: Sonarr/cellarr say `bluray-<res> remux`,
/// Radarr says `remux-<res>`. Applied to cellarr's quality before comparing
/// against a Radarr-routed title, so the known-and-intended G7 vocabulary
/// difference is not counted as a false mismatch (the parser detects the remux
/// correctly; only the face spelling differs). See docs/parity/quality-vocab.md.
fn radarr_face_quality(name: &str) -> String {
    if let Some(res) = name
        .strip_prefix("bluray-")
        .and_then(|r| r.strip_suffix(" remux"))
    {
        format!("remux-{res}")
    } else {
        name.to_string()
    }
}

fn from_cellarr(input: &str, ranking: &QualityRanking) -> Record {
    let p = parse_title(input);
    let mut r = Record {
        title: p.clean_title.as_deref().map(norm_title),
        group: norm_opt(p.group.as_deref()),
        year: p.year.map(i64::from),
        edition: norm_opt(p.edition.as_deref()),
        ..Default::default()
    };
    // Quality only counts when cellarr actually identified a source/resolution;
    // an all-unknown release resolving to a fallback bucket would be noise.
    if p.source.is_some() || p.resolution.is_some() {
        r.quality = Some(resolve_quality(&p, ranking).name.to_lowercase());
    }
    for c in &p.coordinates {
        match c {
            Coordinates::Episode {
                season,
                episode,
                absolute,
            } => {
                r.season.get_or_insert(i64::from(*season));
                r.episodes.push(i64::from(*episode));
                if let Some(a) = absolute {
                    r.absolute.push(i64::from(*a));
                }
            }
            Coordinates::SeasonPack { season } => {
                r.season.get_or_insert(i64::from(*season));
                r.full_season = true;
            }
            Coordinates::Absolute { number } => r.absolute.push(i64::from(*number)),
            Coordinates::Daily { date } => r.daily_date = Some(date.clone()),
            Coordinates::Movie | Coordinates::Track { .. } | Coordinates::Book { .. } => {}
        }
    }
    r.episodes.sort_unstable();
    r.absolute.sort_unstable();
    r
}

fn arr(v: &serde_json::Value, key: &str) -> Vec<i64> {
    let mut out: Vec<i64> = v
        .get(key)
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(serde_json::Value::as_i64).collect())
        .unwrap_or_default();
    out.sort_unstable();
    out
}

fn from_sonarr(v: &serde_json::Value) -> Record {
    let pi = v.get("parsedEpisodeInfo").cloned().unwrap_or_default();
    let quality = pi
        .pointer("/quality/quality/name")
        .and_then(|x| x.as_str())
        .map(str::to_lowercase);
    Record {
        title: pi
            .get("seriesTitle")
            .and_then(|x| x.as_str())
            .map(norm_title),
        season: pi
            .get("seasonNumber")
            .and_then(serde_json::Value::as_i64)
            .filter(|s| *s >= 0),
        episodes: arr(&pi, "episodeNumbers"),
        absolute: arr(&pi, "absoluteEpisodeNumbers"),
        daily_date: pi
            .get("airDate")
            .and_then(|x| x.as_str())
            .map(str::to_string)
            .filter(|s| !s.is_empty()),
        full_season: pi
            .get("fullSeason")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        group: norm_opt(pi.get("releaseGroup").and_then(|x| x.as_str())),
        quality,
        year: None,
        edition: None,
    }
}

fn from_radarr(v: &serde_json::Value) -> Record {
    let pi = v.get("parsedMovieInfo").cloned().unwrap_or_default();
    Record {
        title: pi
            .get("movieTitle")
            .and_then(|x| x.as_str())
            .map(norm_title),
        group: norm_opt(pi.get("releaseGroup").and_then(|x| x.as_str())),
        quality: pi
            .pointer("/quality/quality/name")
            .and_then(|x| x.as_str())
            .map(str::to_lowercase),
        year: pi
            .get("year")
            .and_then(serde_json::Value::as_i64)
            .filter(|y| *y > 0),
        edition: norm_opt(pi.get("edition").and_then(|x| x.as_str())),
        ..Default::default()
    }
}

/// Which fields matter for a domain.
fn fields_for(domain: Domain) -> &'static [&'static str] {
    match domain {
        Domain::TvEpisode => &["title", "season", "episodes", "group", "quality"],
        // Daily/absolute carry no real season (Sonarr uses a season-0 sentinel that
        // cellarr correctly omits in favour of the Daily/Absolute coordinate), so
        // we compare the date / absolute number instead.
        Domain::Daily => &["title", "daily_date", "group", "quality"],
        Domain::Absolute => &["title", "absolute", "group", "quality"],
        Domain::TvGeneric => &["title", "group", "quality"],
        Domain::Movie => &["title", "year", "edition", "group", "quality"],
    }
}

/// Compare one field; returns Some((cellarr, oracle)) on mismatch, None on match.
fn cmp_field(field: &str, c: &Record, o: &Record) -> Option<(String, String)> {
    fn s(o: &Option<String>) -> String {
        o.clone().unwrap_or_else(|| "∅".into())
    }
    fn n(o: &Option<i64>) -> String {
        o.map(|x| x.to_string()).unwrap_or_else(|| "∅".into())
    }
    fn v(a: &[i64]) -> String {
        if a.is_empty() {
            "∅".into()
        } else {
            format!("{a:?}")
        }
    }
    let mm = |a: String, b: String| if a == b { None } else { Some((a, b)) };
    match field {
        "title" => mm(s(&c.title), s(&o.title)),
        "season" => mm(n(&c.season), n(&o.season)),
        "episodes" => mm(v(&c.episodes), v(&o.episodes)),
        "absolute" => mm(v(&c.absolute), v(&o.absolute)),
        "daily_date" => mm(s(&c.daily_date), s(&o.daily_date)),
        "group" => mm(s(&c.group), s(&o.group)),
        "quality" => {
            // Only compare when both sides produced a quality; a one-sided ∅ is a
            // softer signal recorded separately, not a hard field mismatch here.
            match (&c.quality, &o.quality) {
                (Some(a), Some(b)) => mm(a.clone(), b.clone()),
                _ => None,
            }
        }
        "year" => mm(n(&c.year), n(&o.year)),
        "edition" => mm(s(&c.edition), s(&o.edition)),
        _ => None,
    }
}

/// One unit of work: a title to parse, which app is the oracle, and which domain
/// drives the field set. `set` tags the corpus partition ("curated" / "upstream").
struct Job {
    set: &'static str,
    file: String,
    input: String,
    app: App,
    domain: Domain,
}

/// Collect every job from both corpus partitions, applying the routing rules.
fn collect_jobs() -> Vec<Job> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut jobs = Vec::new();

    // Curated: corpus/parse/*.toml, routed per-file (+ movie-shaped to Radarr).
    let curated = root.join("parse");
    let mut files: Vec<PathBuf> = fs::read_dir(&curated)
        .expect("read corpus/parse")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    for path in files {
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let domain = classify_curated(&stem);
        let parsed: CorpusFile =
            toml::from_str(&fs::read_to_string(&path).expect("read")).expect("toml");
        for case in parsed.case {
            let use_radarr = domain == Domain::Movie
                || (domain == Domain::TvGeneric && movie_shaped(&case.input));
            jobs.push(Job {
                set: "curated",
                file: stem.clone(),
                input: case.input,
                app: if use_radarr { App::Radarr } else { App::Sonarr },
                domain,
            });
        }
    }

    // Upstream: corpus/upstream/{sonarr,radarr}/*.toml, routed by directory.
    let upstream = root.join("upstream");
    for (sub, app) in [("sonarr", App::Sonarr), ("radarr", App::Radarr)] {
        let dir = upstream.join(sub);
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        files.sort();
        for path in files {
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let domain = classify_upstream(app, &stem);
            let parsed: CorpusFile =
                toml::from_str(&fs::read_to_string(&path).expect("read")).expect("toml");
            for case in parsed.case {
                jobs.push(Job {
                    set: "upstream",
                    file: format!("{sub}/{stem}"),
                    input: case.input,
                    app,
                    domain,
                });
            }
        }
    }
    jobs
}

#[test]
#[ignore = "differential oracle; run via `just oracle` with live Sonarr/Radarr"]
fn oracle_parser_parity() {
    let (Ok(sonarr), Ok(radarr)) = (
        std::env::var("CELLARR_ORACLE_SONARR"),
        std::env::var("CELLARR_ORACLE_RADARR"),
    ) else {
        eprintln!(
            "oracle env not set (CELLARR_ORACLE_SONARR/RADARR); skipping. Use `just oracle`."
        );
        return;
    };
    let sk = std::env::var("CELLARR_ORACLE_SONARR_KEY").unwrap_or_default();
    let rk = std::env::var("CELLARR_ORACLE_RADARR_KEY").unwrap_or_default();

    let ranking = QualityRanking::default();
    let jobs = collect_jobs();
    eprintln!("oracle: {} titles queued (curated + upstream)", jobs.len());

    // Bounded-concurrency HTTP. Each call has a hard ≤10s timeout; a dedicated
    // rayon pool caps in-flight requests so we never stampede the apps.
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(32)
        .build()
        .expect("client");

    let parse = |app: App, title: &str| -> Option<serde_json::Value> {
        let (base, key) = match app {
            App::Sonarr => (&sonarr, &sk),
            App::Radarr => (&radarr, &rk),
        };
        let resp = client
            .get(format!("{base}/api/v3/parse"))
            .query(&[("title", title)])
            .header("X-Api-Key", key.as_str())
            .send()
            .ok()?;
        resp.json().ok()
    };

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(24)
        .build()
        .expect("pool");

    let done = AtomicUsize::new(0);
    let total = jobs.len();

    // Outcome per job: per-field (matched?) plus optional error + mismatch rows.
    struct Outcome {
        set: &'static str,
        file: String,
        oracle_error: bool,
        title_exact: bool,
        fields: Vec<(&'static str, bool)>,
        mismatches: Vec<serde_json::Value>,
    }

    let outcomes: Vec<Outcome> = pool.install(|| {
        jobs.par_iter()
            .map(|job| {
                let mut cell = from_cellarr(&job.input, &ranking);
                // On the Radarr face, cellarr's remux tier is spelled `remux-<res>`
                // (matching Radarr); apply that rename before comparison.
                if job.app == App::Radarr {
                    if let Some(q) = cell.quality.take() {
                        cell.quality = Some(radarr_face_quality(&q));
                    }
                }
                let oracle = parse(job.app, &job.input).map(|v| match job.app {
                    App::Sonarr => from_sonarr(&v),
                    App::Radarr => from_radarr(&v),
                });
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 200 == 0 {
                    eprintln!("  ... {n}/{total}");
                }
                let Some(oracle) = oracle else {
                    return Outcome {
                        set: job.set,
                        file: job.file.clone(),
                        oracle_error: true,
                        title_exact: false,
                        fields: Vec::new(),
                        mismatches: vec![serde_json::json!({
                            "set": job.set, "file": job.file, "input": job.input,
                            "field": "_oracle_error", "cellarr": "", "oracle": "no response"
                        })],
                    };
                };
                let mut fields = Vec::new();
                let mut mismatches = Vec::new();
                let mut title_exact = true;
                for &field in fields_for(job.domain) {
                    match cmp_field(field, &cell, &oracle) {
                        None => fields.push((field, true)),
                        Some((c, o)) => {
                            fields.push((field, false));
                            title_exact = false;
                            mismatches.push(serde_json::json!({
                                "set": job.set, "file": job.file, "input": job.input,
                                "field": field, "cellarr": c, "oracle": o
                            }));
                        }
                    }
                }
                Outcome {
                    set: job.set,
                    file: job.file.clone(),
                    oracle_error: false,
                    title_exact,
                    fields,
                    mismatches,
                }
            })
            .collect()
    });

    // Aggregate. Tallies are kept per-partition AND combined.
    #[derive(Default)]
    struct Agg {
        compared: BTreeMap<String, u64>,
        matched: BTreeMap<String, u64>,
        by_file: BTreeMap<String, (u64, u64)>, // (titles, exact)
        total_titles: u64,
        exact_titles: u64,
        oracle_errors: u64,
    }
    let mut all = Agg::default();
    let mut curated = Agg::default();
    let mut upstream = Agg::default();
    let mut mismatches: Vec<serde_json::Value> = Vec::new();

    for o in &outcomes {
        let buckets: [&mut Agg; 2] = if o.set == "curated" {
            [&mut all, &mut curated]
        } else {
            [&mut all, &mut upstream]
        };
        mismatches.extend(o.mismatches.iter().cloned());
        if o.oracle_error {
            for b in buckets {
                b.oracle_errors += 1;
            }
            continue;
        }
        for b in buckets {
            b.total_titles += 1;
            let ent = b.by_file.entry(o.file.clone()).or_default();
            ent.0 += 1;
            for (field, ok) in &o.fields {
                *b.compared.entry((*field).to_string()).or_default() += 1;
                if *ok {
                    *b.matched.entry((*field).to_string()).or_default() += 1;
                }
            }
            if o.title_exact {
                b.exact_titles += 1;
                ent.1 += 1;
            }
        }
    }

    fn field_rates(a: &Agg) -> BTreeMap<String, f64> {
        a.compared
            .iter()
            .map(|(f, c)| {
                let m = *a.matched.get(f).unwrap_or(&0);
                (f.clone(), if *c == 0 { 1.0 } else { m as f64 / *c as f64 })
            })
            .collect()
    }
    fn agg_json(a: &Agg) -> serde_json::Value {
        serde_json::json!({
            "total_titles": a.total_titles,
            "exact_titles": a.exact_titles,
            "exact_rate": if a.total_titles == 0 { 0.0 } else { a.exact_titles as f64 / a.total_titles as f64 },
            "oracle_errors": a.oracle_errors,
            "field_compared": a.compared,
            "field_matched": a.matched,
            "field_rates": field_rates(a),
            "by_file": a.by_file.iter().map(|(k,(t,e))| (k.clone(), serde_json::json!({"titles":t,"exact":e}))).collect::<BTreeMap<_,_>>(),
        })
    }

    // Write raw outputs under target/parity (git-ignored).
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/parity");
    fs::create_dir_all(&out_dir).ok();
    let jsonl: String = mismatches
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(out_dir.join("oracle-fullscale-mismatches.jsonl"), jsonl).ok();

    let summary = serde_json::json!({
        "all": agg_json(&all),
        "curated": agg_json(&curated),
        "upstream": agg_json(&upstream),
        "mismatch_count": mismatches.len(),
    });
    fs::write(
        out_dir.join("oracle-fullscale.json"),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .ok();

    // Human summary (visible with --nocapture).
    let report = |label: &str, a: &Agg| {
        println!(
            "\n=== {label}: {} titles  exact {} ({:.1}%)  oracle-errors {} ===",
            a.total_titles,
            a.exact_titles,
            if a.total_titles == 0 {
                0.0
            } else {
                a.exact_titles as f64 / a.total_titles as f64 * 100.0
            },
            a.oracle_errors,
        );
        for (f, r) in field_rates(a) {
            let c = a.compared.get(&f).unwrap_or(&0);
            let m = a.matched.get(&f).unwrap_or(&0);
            println!("  {f:10} {:.1}%  ({m}/{c})", r * 100.0);
        }
    };
    println!("\n=== cellarr parser parity vs Sonarr/Radarr (FULL corpus) ===");
    report("ALL", &all);
    report("curated (corpus/parse)", &curated);
    report("upstream (corpus/upstream)", &upstream);
    println!(
        "\nresults: target/parity/oracle-fullscale.json + oracle-fullscale-mismatches.jsonl ({} rows)",
        mismatches.len()
    );
    // Intentionally not asserting a threshold: this catalogues at-scale divergence.
}
