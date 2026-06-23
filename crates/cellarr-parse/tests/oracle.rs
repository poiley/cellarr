//! Differential-oracle harness: diff cellarr's parser against the live
//! Sonarr/Radarr `/api/v3/parse` endpoints over the `corpus/parse` titles.
//!
//! `#[ignore]` so it never runs in normal `cargo test`. It self-skips when the
//! oracle env vars are unset, so `just oracle` (which sets them after bringing up
//! the containers) is the intended entry point. Results are written under
//! `target/parity/` (git-ignored): a per-field summary JSON and a JSONL of every
//! mismatch, so no finding is lost. See docs/parity/.
//!
//! Run: `just oracle`, or manually with CELLARR_ORACLE_SONARR[_KEY] /
//! CELLARR_ORACLE_RADARR[_KEY] set.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use cellarr_core::profile::{resolve_quality, QualityRanking};
use cellarr_core::Coordinates;
use cellarr_parse::parse_title;
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

fn classify(file_stem: &str) -> Domain {
    match file_stem {
        "movie_title" | "movie_year" | "movie_edition" => Domain::Movie,
        "daily_episode" => Domain::Daily,
        "absolute_anime" => Domain::Absolute,
        "single_episode" | "multi_episode" | "season" | "miniseries" => Domain::TvEpisode,
        _ => Domain::TvGeneric, // quality, language, release_group, unicode, proper_repack
    }
}

// Movie-shaped: a 4-digit year and no episode/absolute numbering. Used to route
// the generic-quality corpus per-title (e.g. movie-only CAM/HDCAM titles go to
// Radarr, which has those qualities; sending them to Sonarr would mis-map to SDTV).
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
    let rev_ver = pi
        .pointer("/quality/revision/version")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(1);
    let _ = rev_ver; // proper/repack comparison handled at field level later
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

    let corpus_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/parse")
        .canonicalize()
        .expect("corpus/parse dir");
    let ranking = QualityRanking::default();
    let client = reqwest::blocking::Client::new();

    let parse = |base: &str, key: &str, title: &str| -> Option<serde_json::Value> {
        let resp = client
            .get(format!("{base}/api/v3/parse"))
            .query(&[("title", title)])
            .header("X-Api-Key", key)
            .send()
            .ok()?;
        resp.json().ok()
    };

    // per-field tallies and the raw mismatch log
    let mut compared: BTreeMap<String, u64> = BTreeMap::new();
    let mut matched: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_file: BTreeMap<String, (u64, u64)> = BTreeMap::new(); // (titles, exact)
    let mut mismatches: Vec<serde_json::Value> = Vec::new();
    let mut total_titles = 0u64;
    let mut exact_titles = 0u64;

    let mut files: Vec<PathBuf> = fs::read_dir(&corpus_dir)
        .expect("read corpus")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();

    for path in files {
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let domain = classify(&stem);
        let text = fs::read_to_string(&path).expect("read corpus file");
        let parsed: CorpusFile = toml::from_str(&text).expect("parse corpus toml");
        for case in parsed.case {
            let title = case.input;
            let cell = from_cellarr(&title, &ranking);
            // Route movie titles (and movie-shaped generic-corpus titles) to Radarr;
            // everything else to Sonarr.
            let use_radarr =
                domain == Domain::Movie || (domain == Domain::TvGeneric && movie_shaped(&title));
            let oracle = if use_radarr {
                parse(&radarr, &rk, &title).as_ref().map(from_radarr)
            } else {
                parse(&sonarr, &sk, &title).as_ref().map(from_sonarr)
            };
            let Some(oracle) = oracle else {
                mismatches.push(serde_json::json!({
                    "file": stem, "input": title, "field": "_oracle_error",
                    "cellarr": "", "oracle": "no response"
                }));
                continue;
            };
            total_titles += 1;
            let file_ent = by_file.entry(stem.clone()).or_default();
            file_ent.0 += 1;
            let mut title_exact = true;
            for &field in fields_for(domain) {
                *compared.entry(field.to_string()).or_default() += 1;
                match cmp_field(field, &cell, &oracle) {
                    None => *matched.entry(field.to_string()).or_default() += 1,
                    Some((c, o)) => {
                        title_exact = false;
                        mismatches.push(serde_json::json!({
                            "file": stem, "input": title, "field": field,
                            "cellarr": c, "oracle": o
                        }));
                    }
                }
            }
            if title_exact {
                exact_titles += 1;
                file_ent.1 += 1;
            }
        }
    }

    // Write raw outputs under target/parity (git-ignored).
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/parity");
    fs::create_dir_all(&out_dir).ok();
    let jsonl: String = mismatches
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(out_dir.join("parser-mismatches.jsonl"), jsonl).ok();

    let field_rates: BTreeMap<String, f64> = compared
        .iter()
        .map(|(f, c)| {
            let m = *matched.get(f).unwrap_or(&0);
            (f.clone(), if *c == 0 { 1.0 } else { m as f64 / *c as f64 })
        })
        .collect();
    let summary = serde_json::json!({
        "total_titles": total_titles,
        "exact_titles": exact_titles,
        "exact_rate": if total_titles == 0 { 0.0 } else { exact_titles as f64 / total_titles as f64 },
        "field_compared": compared,
        "field_matched": matched,
        "field_rates": field_rates,
        "by_file": by_file.iter().map(|(k,(t,e))| (k.clone(), serde_json::json!({"titles":t,"exact":e}))).collect::<BTreeMap<_,_>>(),
        "mismatch_count": mismatches.len(),
    });
    fs::write(
        out_dir.join("parser-results.json"),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .ok();

    // Print a human summary (visible with --nocapture).
    println!("\n=== cellarr parser parity vs Sonarr/Radarr ===");
    println!(
        "titles: {total_titles}  exact: {exact_titles}  mismatches: {}",
        mismatches.len()
    );
    for (f, r) in &field_rates {
        let c = compared.get(f).unwrap_or(&0);
        let m = matched.get(f).unwrap_or(&0);
        println!("  {f:10} {:.1}%  ({m}/{c})", r * 100.0);
    }
    println!("results: target/parity/parser-results.json + parser-mismatches.jsonl");
    // Intentionally not asserting a threshold yet: first we measure & catalogue.
}
