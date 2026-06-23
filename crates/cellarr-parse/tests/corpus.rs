//! Table-driven corpus test.
//!
//! Loads every `corpus/parse/*.toml` vector and runs it through
//! [`cellarr_parse::parse_title`], asserting on the fields each vector pins.
//! Only the fields a vector declares are checked, so a vector can pin a single
//! slice (e.g. just the codec) without asserting everything.
//!
//! The corpus is the parser's acceptance test: every vector records its
//! provenance in `source`, and the file grouping mirrors the concern under test.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use cellarr_core::media::Coordinates;
use cellarr_core::parsed::{
    HdrFormat, ParsedRelease, ProperRepack, Resolution, Source, VideoCodec,
};
use serde::Deserialize;

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

/// Mirror of the subset of `ParsedRelease` fields a corpus vector may assert.
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
    // Numbering. A vector may pin a single season+episode, a list of episodes,
    // an absolute (anime) number, or just a season (season pack).
    season: Option<u32>,
    episode: Option<u32>,
    #[serde(default)]
    episodes: Option<Vec<u32>>,
    absolute: Option<u32>,
    season_pack: Option<bool>,
    daily: Option<bool>,
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/parse")
        .canonicalize()
        .expect("corpus/parse dir exists")
}

fn parse_resolution(s: &str) -> Resolution {
    match s {
        "480p" => Resolution::R480p,
        "576p" => Resolution::R576p,
        "720p" => Resolution::R720p,
        "1080p" => Resolution::R1080p,
        "2160p" => Resolution::R2160p,
        other => panic!("corpus resolution `{other}` not recognised"),
    }
}

fn parse_source(s: &str) -> Source {
    match s {
        "cam" => Source::Cam,
        "sdtv" => Source::Sdtv,
        "hdtv" => Source::Hdtv,
        "webrip" => Source::Webrip,
        "web-dl" => Source::WebDl,
        "dvd" => Source::Dvd,
        "bluray" => Source::Bluray,
        "remux" => Source::Remux,
        other => panic!("corpus source `{other}` not recognised"),
    }
}

fn parse_codec(s: &str) -> VideoCodec {
    match s {
        "x264" => VideoCodec::X264,
        "x265" => VideoCodec::X265,
        "av1" => VideoCodec::Av1,
        "other" => VideoCodec::Other,
        other => panic!("corpus codec `{other}` not recognised"),
    }
}

fn parse_pr(s: &str) -> ProperRepack {
    match s {
        "proper" => ProperRepack::Proper,
        "repack" => ProperRepack::Repack,
        other => panic!("corpus proper_repack `{other}` not recognised"),
    }
}

fn parse_hdr(s: &str) -> HdrFormat {
    match s {
        "hdr10" => HdrFormat::Hdr10,
        "hdr10plus" => HdrFormat::Hdr10Plus,
        "dolbyvision" => HdrFormat::DolbyVision,
        "hlg" => HdrFormat::Hlg,
        other => panic!("corpus hdr `{other}` not recognised"),
    }
}

/// Check one case, returning `Ok(())` on a pass or `Err(reason)` on a mismatch.
fn check(case: &Case, got: &ParsedRelease) -> Result<(), String> {
    let e = &case.expected;

    if let Some(t) = &e.title {
        let actual = got.clean_title.as_deref().unwrap_or("");
        if !title_eq(actual, t) {
            return Err(format!("title: want {t:?}, got {actual:?}"));
        }
    }
    if let Some(y) = e.year {
        if got.year != Some(y) {
            return Err(format!("year: want {y:?}, got {:?}", got.year));
        }
    }
    if let Some(r) = &e.resolution {
        let want = parse_resolution(r);
        if got.resolution != Some(want) {
            return Err(format!(
                "resolution: want {want:?}, got {:?}",
                got.resolution
            ));
        }
    }
    if let Some(s) = &e.source {
        let want = parse_source(s);
        if got.source != Some(want) {
            return Err(format!("source: want {want:?}, got {:?}", got.source));
        }
    }
    if let Some(c) = &e.codec {
        let want = parse_codec(c);
        if got.codec != Some(want) {
            return Err(format!("codec: want {want:?}, got {:?}", got.codec));
        }
    }
    if let Some(g) = &e.group {
        if got.group.as_deref() != Some(g.as_str()) {
            return Err(format!("group: want {g:?}, got {:?}", got.group));
        }
    }
    if let Some(pr) = &e.proper_repack {
        let want = parse_pr(pr);
        if got.proper_repack != Some(want) {
            return Err(format!(
                "proper_repack: want {want:?}, got {:?}",
                got.proper_repack
            ));
        }
    }
    if let Some(ed) = &e.edition {
        if got.edition.as_deref() != Some(ed.as_str()) {
            return Err(format!("edition: want {ed:?}, got {:?}", got.edition));
        }
    }
    if let Some(langs) = &e.languages {
        for want in langs {
            if !got.languages.iter().any(|l| l == want) {
                return Err(format!(
                    "languages: want to contain {want:?}, got {:?}",
                    got.languages
                ));
            }
        }
    }
    if let Some(audio) = &e.audio {
        for want in audio {
            if !got.audio.iter().any(|a| a == want) {
                return Err(format!(
                    "audio: want to contain {want:?}, got {:?}",
                    got.audio
                ));
            }
        }
    }
    if let Some(hdr) = &e.hdr {
        for want in hdr {
            let w = parse_hdr(want);
            if !got.hdr.contains(&w) {
                return Err(format!("hdr: want to contain {w:?}, got {:?}", got.hdr));
            }
        }
    }

    check_numbering(e, got)?;
    Ok(())
}

fn check_numbering(e: &Expected, got: &ParsedRelease) -> Result<(), String> {
    // Anime absolute.
    if let Some(abs) = e.absolute {
        let found = got
            .coordinates
            .iter()
            .any(|c| matches!(c, Coordinates::Episode { absolute: Some(a), .. } if *a == abs));
        if !found {
            return Err(format!("absolute: want {abs}, got {:?}", got.coordinates));
        }
        return Ok(());
    }

    // Daily: no season/episode coordinates surfaced (date addressing).
    if e.daily == Some(true) {
        if !got.coordinates.is_empty() {
            return Err(format!(
                "daily: want no coordinates, got {:?}",
                got.coordinates
            ));
        }
        return Ok(());
    }

    // Explicit multi-episode list.
    if let Some(eps) = &e.episodes {
        let season = e.season.unwrap_or(0);
        let actual: Vec<u32> = got
            .coordinates
            .iter()
            .filter_map(|c| match c {
                Coordinates::Episode {
                    season: s, episode, ..
                } if *s == season => Some(*episode),
                _ => None,
            })
            .collect();
        if &actual != eps {
            return Err(format!("episodes: want {eps:?}, got {actual:?}"));
        }
        return Ok(());
    }

    // Season pack: a single season coordinate with episode 0 sentinel.
    if e.season_pack == Some(true) {
        let season = e.season.unwrap_or(0);
        let found = got.coordinates.iter().any(
            |c| matches!(c, Coordinates::Episode { season: s, episode: 0, .. } if *s == season),
        );
        if !found {
            return Err(format!(
                "season_pack: want season {season} ep 0, got {:?}",
                got.coordinates
            ));
        }
        return Ok(());
    }

    // Single episode.
    if let (Some(s), Some(ep)) = (e.season, e.episode) {
        let found = got.coordinates.iter().any(|c| {
            matches!(c, Coordinates::Episode { season, episode, .. } if *season == s && *episode == ep)
        });
        if !found {
            return Err(format!(
                "episode: want S{s}E{ep}, got {:?}",
                got.coordinates
            ));
        }
    }
    Ok(())
}

/// Title comparison tolerant of trailing punctuation differences.
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

#[test]
fn corpus_parse_vectors() {
    let dir = corpus_dir();
    let mut total = 0usize;
    let mut passed = 0usize;
    let mut failures: BTreeMap<String, Vec<String>> = BTreeMap::new();

    let mut files: Vec<_> = fs::read_dir(&dir)
        .expect("read corpus dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("toml"))
        .collect();
    files.sort();

    assert!(!files.is_empty(), "no corpus/parse/*.toml vectors found");

    for file in &files {
        let text = fs::read_to_string(file).expect("read corpus file");
        let parsed: CorpusFile =
            toml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", file.display()));
        let fname = file.file_name().unwrap().to_string_lossy().into_owned();
        for case in &parsed.case {
            total += 1;
            let got = cellarr_parse::parse_title(&case.input);
            match check(case, &got) {
                Ok(()) => passed += 1,
                Err(reason) => failures
                    .entry(fname.clone())
                    .or_default()
                    .push(format!("  {:?}\n    {reason}", case.input)),
            }
        }
    }

    eprintln!("corpus: {passed}/{total} vectors pass");
    if !failures.is_empty() {
        let mut msg = format!("\n{} corpus vectors failed:\n", total - passed);
        for (file, fails) in &failures {
            msg.push_str(&format!("[{file}]\n"));
            for f in fails {
                msg.push_str(f);
                msg.push('\n');
            }
        }
        panic!("{msg}");
    }
}
