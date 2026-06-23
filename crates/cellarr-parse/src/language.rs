//! Language extraction.
//!
//! Languages appear as full names (`French`), scene tags (`MULTi`, `VOSTFR`),
//! and occasionally country/dub markers. We collect the recognised tokens, in a
//! deterministic (sorted, de-duplicated) order, as the names found.

use std::sync::LazyLock;

use cellarr_core::parsed::{Confidence, ParsedField, ParsedRelease};
use regex::Regex;

/// `(pattern, canonical name)`. English is intentionally not auto-added when
/// nothing is present; absence of a language tag is left as an empty list.
const PATTERNS: &[(&str, &str)] = &[
    (r"(?i)\bmulti\b", "Multi"),
    (r"(?i)\b(vostfr|vost)\b", "VOSTFR"),
    (r"(?i)\b(truefrench|vff|vf2|vfq|vfi|vff|french)\b", "French"),
    (r"(?i)\b(german|deutsch|ger)\b", "German"),
    (r"(?i)\b(spanish|castellano|espanol|esp)\b", "Spanish"),
    (r"(?i)\b(italian|ita)\b", "Italian"),
    (r"(?i)\b(japanese|jpn|jap)\b", "Japanese"),
    (r"(?i)\b(korean|kor)\b", "Korean"),
    (r"(?i)\b(chinese|chi|mandarin|cantonese)\b", "Chinese"),
    (r"(?i)\b(russian|rus)\b", "Russian"),
    (
        r"(?i)\b(portuguese|brazilian|dublado|legendado)\b",
        "Portuguese",
    ),
    (r"(?i)\b(dutch|nl)\b", "Dutch"),
    (
        r"(?i)\b(nordic|swedish|danish|norwegian|finnish)\b",
        "Nordic",
    ),
    (r"(?i)\b(hindi|tamil|telugu)\b", "Hindi"),
    (r"(?i)\b(english|eng)\b", "English"),
];

static REGEXES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    PATTERNS
        .iter()
        .map(|(p, c)| (Regex::new(p).unwrap(), *c))
        .collect()
});

/// Extract language tags.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    let norm = crate::tokens::normalize(input);
    let mut langs: Vec<String> = Vec::new();
    for (re, canon) in REGEXES.iter() {
        if re.is_match(&norm) {
            langs.push((*canon).to_owned());
        }
    }
    if langs.is_empty() {
        return;
    }
    langs.sort();
    langs.dedup();
    out.languages = langs;
    out.set_confidence(ParsedField::Languages, Confidence::new(0.7));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lang(s: &str) -> Vec<String> {
        let mut p = ParsedRelease::new(s);
        extract(s, &mut p);
        p.languages
    }

    #[test]
    fn multi_and_french() {
        let l = lang("Movie.2019.MULTi.VFF.1080p.BluRay");
        assert!(l.contains(&"Multi".to_string()));
        assert!(l.contains(&"French".to_string()));
    }

    #[test]
    fn vostfr() {
        assert!(lang("Movie.2019.VOSTFR.1080p").contains(&"VOSTFR".to_string()));
    }

    #[test]
    fn none_when_absent() {
        assert!(lang("Show.S01E01.1080p.WEB-DL.x264-GRP").is_empty());
    }
}
