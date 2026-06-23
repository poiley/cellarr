//! Drives the rename engine against the `corpus/naming/*.toml` vectors.
//!
//! The corpus is the language-neutral spec for on-disk naming: every
//! `format + tokens -> expected.path` fact is asserted here and, separately, by
//! the differential oracle. A failure here reads as a naming-spec violation.

use std::collections::BTreeMap;
use std::path::PathBuf;

use cellarr_core::NamingTokens;
use cellarr_fs::render_name;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct CorpusFile {
    #[serde(default)]
    case: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    source: String,
    #[serde(default)]
    notes: Option<String>,
    format: String,
    tokens: BTreeMap<String, String>,
    expected: Expected,
}

#[derive(Debug, Deserialize)]
struct Expected {
    path: String,
}

fn corpus_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/cellarr-fs; the corpus is at the repo root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("naming")
}

fn load_cases() -> Vec<(String, Case)> {
    let dir = corpus_dir();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("corpus/naming should exist") {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let parsed: CorpusFile =
            toml::from_str(&text).unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()));
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        for case in parsed.case {
            out.push((file.clone(), case));
        }
    }
    out
}

#[test]
fn every_naming_vector_renders_to_its_expected_path() {
    let cases = load_cases();
    assert!(
        cases.len() >= 10,
        "expected a populated naming corpus, found {} cases",
        cases.len()
    );

    for (file, case) in cases {
        // Every vector must carry provenance.
        assert!(
            !case.source.trim().is_empty(),
            "{file}: a vector is missing its `source` provenance"
        );

        let tokens = NamingTokens {
            tokens: case.tokens.into_iter().collect(),
        };
        let rendered = render_name(&case.format, &tokens).unwrap_or_else(|e| {
            panic!(
                "{file}: render failed for format {:?}: {e}\nnotes: {:?}",
                case.format, case.notes
            )
        });
        assert_eq!(
            rendered, case.expected.path,
            "{file}: format {:?} (notes: {:?})",
            case.format, case.notes
        );
    }
}
