//! Measured self-parity against the harvested upstream corpus.
//!
//! Loads every `corpus/upstream/**/*.toml` vector (re-curated input->expected
//! FACTS extracted clean-room from Sonarr/Radarr's GPLv3 parser test fixtures —
//! see docs/agents/legal-and-licensing.md), runs cellarr's parser on each input,
//! and compares the fields each vector asserts. It computes a per-field and an
//! overall pass rate, writes the results under `target/parity/` (git-ignored),
//! and **asserts the overall rate stays at or above a ratchet floor**.
//!
//! Unlike `corpus.rs` (the curated 100%-must-pass set), the upstream set is the
//! originals' own fixtures verbatim-as-facts: it intentionally contains cases
//! where cellarr deliberately diverges (canonicalised editions, library-locked
//! anime, Part-N miniseries). So the floor is the achieved rate, not 100%. The
//! threshold is a ratchet: raise it as the parser improves, never lower it.
//!
//! This is `cargo test` (no Docker, no live apps): it is the static counterpart
//! to the differential `oracle.rs` harness.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use cellarr_core::media::Coordinates;
use cellarr_core::parsed::{
    HdrFormat, ParsedRelease, ProperRepack, Resolution, Source, VideoCodec,
};
use serde::Deserialize;

/// The achieved overall field pass-rate floor. Ratchet UP as fixes land; never
/// lower. Achieved this pass: 0.6564 (880/1555 cases exact). The floor sits just
/// under that so a small environment/measurement jitter cannot red CI, but any
/// real regression trips it. See docs/parity/PARITY_REPORT.md for the history.
const RATCHET_OVERALL: f64 = 0.65;

#[derive(Debug, Deserialize)]
struct CorpusFile {
    #[serde(default)]
    case: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    input: String,
    #[allow(dead_code)]
    source: String,
    #[serde(default)]
    #[allow(dead_code)]
    notes: Option<String>,
    expected: Expected,
}

/// Mirror of the subset of `ParsedRelease` fields an upstream vector may assert.
/// Every field is optional; absent fields are simply not checked.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expected {
    title: Option<String>,
    year: Option<u16>,
    resolution: Option<String>,
    source: Option<String>,
    codec: Option<String>,
    group: Option<String>,
    proper_repack: Option<String>,
    edition: Option<String>,
    #[serde(default)]
    languages: Option<Vec<String>>,
    #[serde(default)]
    audio: Option<Vec<String>>,
    #[serde(default)]
    hdr: Option<Vec<String>>,
    season: Option<u32>,
    episode: Option<u32>,
    #[serde(default)]
    episodes: Option<Vec<u32>>,
    absolute: Option<u32>,
    season_pack: Option<bool>,
    daily: Option<bool>,
}

fn upstream_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/upstream")
        .canonicalize()
        .expect("corpus/upstream dir exists")
}

fn parse_resolution(s: &str) -> Resolution {
    match s {
        "480p" => Resolution::R480p,
        "576p" => Resolution::R576p,
        "720p" => Resolution::R720p,
        "1080p" => Resolution::R1080p,
        "2160p" => Resolution::R2160p,
        other => panic!("upstream resolution `{other}` not recognised"),
    }
}

fn parse_source(s: &str) -> Source {
    match s {
        "workprint" => Source::Workprint,
        "cam" => Source::Cam,
        "telesync" => Source::Telesync,
        "telecine" => Source::Telecine,
        "regional" => Source::Regional,
        "dvdscr" => Source::Dvdscr,
        "sdtv" => Source::Sdtv,
        "hdtv" => Source::Hdtv,
        "raw-hd" => Source::RawHd,
        "webrip" => Source::Webrip,
        "web-dl" => Source::WebDl,
        "dvd" => Source::Dvd,
        "dvd-r" => Source::DvdR,
        "bluray" => Source::Bluray,
        "br-disk" => Source::BrDisk,
        "remux" => Source::Remux,
        other => panic!("upstream source `{other}` not recognised"),
    }
}

fn parse_codec(s: &str) -> VideoCodec {
    match s {
        "x264" => VideoCodec::X264,
        "x265" => VideoCodec::X265,
        "av1" => VideoCodec::Av1,
        "other" => VideoCodec::Other,
        other => panic!("upstream codec `{other}` not recognised"),
    }
}

fn parse_pr(s: &str) -> ProperRepack {
    match s {
        "proper" => ProperRepack::Proper,
        "repack" => ProperRepack::Repack,
        other => panic!("upstream proper_repack `{other}` not recognised"),
    }
}

fn parse_hdr(s: &str) -> HdrFormat {
    match s {
        "hdr10" => HdrFormat::Hdr10,
        "hdr10plus" => HdrFormat::Hdr10Plus,
        "dolbyvision" => HdrFormat::DolbyVision,
        "hlg" => HdrFormat::Hlg,
        other => panic!("upstream hdr `{other}` not recognised"),
    }
}

/// Title comparison tolerant of punctuation/case differences (same normalization
/// as the curated corpus + the differential oracle).
fn title_eq(actual: &str, want: &str) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| c.is_alphanumeric() || *c == ' ')
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    norm(actual) == norm(want)
}

/// One field check: `Ok(None)` if the vector doesn't assert it, `Ok(Some(()))`
/// on a match, `Err((cellarr, oracle))` on a mismatch. The field name is the key
/// the caller tallies and logs under.
struct FieldOutcome {
    field: &'static str,
    cellarr: String,
    oracle: String,
    matched: bool,
}

/// Build the per-field outcomes for one case. Only fields the vector asserts are
/// produced. Numbering collapses to a single synthetic field ("numbering") so a
/// season/episode/absolute/daily/pack assertion counts once.
fn outcomes(case: &Case, got: &ParsedRelease) -> Vec<FieldOutcome> {
    let e = &case.expected;
    let mut out = Vec::new();

    if let Some(t) = &e.title {
        let actual = got.clean_title.clone().unwrap_or_default();
        out.push(FieldOutcome {
            field: "title",
            matched: title_eq(&actual, t),
            cellarr: actual,
            oracle: t.clone(),
        });
    }
    if let Some(y) = e.year {
        out.push(FieldOutcome {
            field: "year",
            matched: got.year == Some(y),
            cellarr: got.year.map(|x| x.to_string()).unwrap_or_else(em),
            oracle: y.to_string(),
        });
    }
    if let Some(r) = &e.resolution {
        let want = parse_resolution(r);
        out.push(FieldOutcome {
            field: "resolution",
            matched: got.resolution == Some(want),
            cellarr: opt_dbg(got.resolution),
            oracle: r.clone(),
        });
    }
    if let Some(s) = &e.source {
        let want = parse_source(s);
        out.push(FieldOutcome {
            field: "source",
            matched: got.source == Some(want),
            cellarr: opt_dbg(got.source),
            oracle: s.clone(),
        });
    }
    if let Some(c) = &e.codec {
        let want = parse_codec(c);
        out.push(FieldOutcome {
            field: "codec",
            matched: got.codec == Some(want),
            cellarr: opt_dbg(got.codec),
            oracle: c.clone(),
        });
    }
    if let Some(g) = &e.group {
        let actual = got.group.clone();
        out.push(FieldOutcome {
            field: "group",
            matched: actual.as_deref() == Some(g.as_str()),
            cellarr: actual.unwrap_or_else(em),
            oracle: g.clone(),
        });
    }
    if let Some(pr) = &e.proper_repack {
        let want = parse_pr(pr);
        out.push(FieldOutcome {
            field: "proper_repack",
            matched: got.proper_repack == Some(want),
            cellarr: got
                .proper_repack
                .map(|x| format!("{x:?}"))
                .unwrap_or_else(em),
            oracle: pr.clone(),
        });
    }
    if let Some(ed) = &e.edition {
        let actual = got.edition.clone();
        out.push(FieldOutcome {
            field: "edition",
            matched: actual.as_deref() == Some(ed.as_str()),
            cellarr: actual.unwrap_or_else(em),
            oracle: ed.clone(),
        });
    }
    if let Some(langs) = &e.languages {
        let ok = langs.iter().all(|w| got.languages.iter().any(|l| l == w));
        out.push(FieldOutcome {
            field: "languages",
            matched: ok,
            cellarr: format!("{:?}", got.languages),
            oracle: format!("{langs:?}"),
        });
    }
    if let Some(audio) = &e.audio {
        let ok = audio.iter().all(|w| got.audio.iter().any(|a| a == w));
        out.push(FieldOutcome {
            field: "audio",
            matched: ok,
            cellarr: format!("{:?}", got.audio),
            oracle: format!("{audio:?}"),
        });
    }
    if let Some(hdr) = &e.hdr {
        let ok = hdr.iter().all(|w| got.hdr.contains(&parse_hdr(w)));
        out.push(FieldOutcome {
            field: "hdr",
            matched: ok,
            cellarr: format!("{:?}", got.hdr),
            oracle: format!("{hdr:?}"),
        });
    }

    if let Some(o) = numbering_outcome(e, got) {
        out.push(o);
    }
    out
}

fn em() -> String {
    "∅".into()
}

fn opt_dbg<T: std::fmt::Debug>(o: Option<T>) -> String {
    o.map(|x| format!("{x:?}")).unwrap_or_else(em)
}

/// Collapse the numbering assertion (whichever shape the vector uses) into one
/// "numbering" outcome, mirroring `corpus.rs::check_numbering`.
fn numbering_outcome(e: &Expected, got: &ParsedRelease) -> Option<FieldOutcome> {
    let mk = |matched: bool, oracle: String| {
        Some(FieldOutcome {
            field: "numbering",
            matched,
            cellarr: format!("{:?}", got.coordinates),
            oracle,
        })
    };

    if let Some(abs) = e.absolute {
        let found = got
            .coordinates
            .iter()
            .any(|c| matches!(c, Coordinates::Absolute { number } if *number == abs));
        return mk(found, format!("absolute={abs}"));
    }
    if e.daily == Some(true) {
        let found = got
            .coordinates
            .iter()
            .any(|c| matches!(c, Coordinates::Daily { .. }));
        return mk(found, "daily".into());
    }
    if let Some(eps) = &e.episodes {
        let season = e.season.unwrap_or(0);
        let mut actual: Vec<u32> = got
            .coordinates
            .iter()
            .filter_map(|c| match c {
                Coordinates::Episode {
                    season: s, episode, ..
                } if *s == season => Some(*episode),
                _ => None,
            })
            .collect();
        actual.sort_unstable();
        let mut want = eps.clone();
        want.sort_unstable();
        return mk(actual == want, format!("S{season} {eps:?}"));
    }
    if e.season_pack == Some(true) {
        let season = u16::try_from(e.season.unwrap_or(0)).unwrap_or(u16::MAX);
        let found = got
            .coordinates
            .iter()
            .any(|c| matches!(c, Coordinates::SeasonPack { season: s } if *s == season));
        return mk(found, format!("season_pack S{season}"));
    }
    if let (Some(s), Some(ep)) = (e.season, e.episode) {
        let found = got.coordinates.iter().any(|c| {
            matches!(c, Coordinates::Episode { season, episode, .. } if *season == s && *episode == ep)
        });
        return mk(found, format!("S{s}E{ep}"));
    }
    None
}

#[test]
fn upstream_self_parity() {
    let dir = upstream_dir();

    let mut files: Vec<PathBuf> = Vec::new();
    // corpus/upstream/{sonarr,radarr}/*.toml
    for sub in fs::read_dir(&dir).expect("read upstream dir") {
        let sub = sub.expect("dir entry").path();
        if sub.is_dir() {
            for f in fs::read_dir(&sub).expect("read upstream subdir") {
                let p = f.expect("file entry").path();
                if p.extension().and_then(|x| x.to_str()) == Some("toml") {
                    files.push(p);
                }
            }
        }
    }
    files.sort();
    assert!(
        !files.is_empty(),
        "no corpus/upstream/**/*.toml vectors found"
    );

    // Per-field tallies and the raw mismatch log.
    let mut compared: BTreeMap<String, u64> = BTreeMap::new();
    let mut matched: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_file: BTreeMap<String, (u64, u64)> = BTreeMap::new(); // (cases, exact)
    let mut mismatches: Vec<serde_json::Value> = Vec::new();
    let mut total_compared = 0u64;
    let mut total_matched = 0u64;
    let mut total_cases = 0u64;
    let mut exact_cases = 0u64;

    for file in &files {
        let rel = file
            .strip_prefix(&dir)
            .unwrap_or(file)
            .to_string_lossy()
            .into_owned();
        let text = fs::read_to_string(file).expect("read upstream file");
        let parsed: CorpusFile =
            toml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", file.display()));
        for case in &parsed.case {
            let got = cellarr_parse::parse_title(&case.input);
            let outs = outcomes(case, &got);
            total_cases += 1;
            let ent = by_file.entry(rel.clone()).or_default();
            ent.0 += 1;
            let mut case_exact = true;
            for o in outs {
                let f = o.field.to_string();
                *compared.entry(f.clone()).or_default() += 1;
                total_compared += 1;
                if o.matched {
                    *matched.entry(f).or_default() += 1;
                    total_matched += 1;
                } else {
                    case_exact = false;
                    mismatches.push(serde_json::json!({
                        "file": rel,
                        "input": case.input,
                        "field": o.field,
                        "cellarr": o.cellarr,
                        "oracle": o.oracle,
                    }));
                }
            }
            if case_exact {
                exact_cases += 1;
                ent.1 += 1;
            }
        }
    }

    let overall = if total_compared == 0 {
        1.0
    } else {
        total_matched as f64 / total_compared as f64
    };
    let exact_rate = if total_cases == 0 {
        1.0
    } else {
        exact_cases as f64 / total_cases as f64
    };

    let field_rates: BTreeMap<String, f64> = compared
        .iter()
        .map(|(f, c)| {
            let m = *matched.get(f).unwrap_or(&0);
            (f.clone(), if *c == 0 { 1.0 } else { m as f64 / *c as f64 })
        })
        .collect();

    // Write raw outputs under target/parity (git-ignored).
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/parity");
    fs::create_dir_all(&out_dir).ok();
    let jsonl: String = mismatches
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(out_dir.join("upstream-mismatches.jsonl"), jsonl).ok();

    let summary = serde_json::json!({
        "total_cases": total_cases,
        "exact_cases": exact_cases,
        "exact_rate": exact_rate,
        "field_compared_total": total_compared,
        "field_matched_total": total_matched,
        "overall_field_rate": overall,
        "ratchet_overall": RATCHET_OVERALL,
        "field_compared": compared,
        "field_matched": matched,
        "field_rates": field_rates,
        "by_file": by_file
            .iter()
            .map(|(k, (c, ex))| (k.clone(), serde_json::json!({"cases": c, "exact": ex})))
            .collect::<BTreeMap<_, _>>(),
        "mismatch_count": mismatches.len(),
    });
    fs::write(
        out_dir.join("upstream-selfparity.json"),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .ok();

    eprintln!("\n=== cellarr upstream self-parity ===");
    eprintln!(
        "cases: {total_cases}  exact: {exact_cases} ({:.1}%)  fields: {total_matched}/{total_compared} = {:.2}%",
        exact_rate * 100.0,
        overall * 100.0
    );
    for (f, r) in &field_rates {
        let c = compared.get(f).unwrap_or(&0);
        let m = matched.get(f).unwrap_or(&0);
        eprintln!("  {f:14} {:.1}%  ({m}/{c})", r * 100.0);
    }
    eprintln!("results: target/parity/upstream-selfparity.json + upstream-mismatches.jsonl");

    assert!(
        overall >= RATCHET_OVERALL,
        "upstream self-parity regressed: overall {:.4} < ratchet {:.4}. \
         Investigate target/parity/upstream-mismatches.jsonl; do NOT lower the ratchet to pass.",
        overall,
        RATCHET_OVERALL
    );
}
