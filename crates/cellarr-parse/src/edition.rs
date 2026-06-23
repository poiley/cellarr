//! Edition extraction (Director's Cut, Extended, IMAX, Theatrical, …).
//!
//! Editions are free-form strings on `ParsedRelease`. The recognised editions
//! are normalised to a canonical spelling; the first match in appearance order
//! wins, since a release advertises one edition.

use std::sync::LazyLock;

use cellarr_core::parsed::{Confidence, ParsedField, ParsedRelease};
use regex::Regex;

const PATTERNS: &[(&str, &str)] = &[
    (r"(?i)\b(director'?s?[\s\-]?cut|dc)\b", "Director's Cut"),
    (r"(?i)\b(final[\s\-]?cut)\b", "Final Cut"),
    (
        r"(?i)\bextended([\s\-]?(cut|edition|version))?\b",
        "Extended",
    ),
    (r"(?i)\b(theatrical([\s\-]?cut)?)\b", "Theatrical"),
    (r"(?i)\b(imax)\b", "IMAX"),
    (r"(?i)\b(unrated)\b", "Unrated"),
    (r"(?i)\b(uncut)\b", "Uncut"),
    (r"(?i)\b(remaster(ed)?)\b", "Remastered"),
    (r"(?i)\b(criterion([\s\-]?collection)?)\b", "Criterion"),
    (r"(?i)\b(special[\s\-]?edition|se)\b", "Special Edition"),
    (r"(?i)\b(ultimate([\s\-]?edition)?)\b", "Ultimate Edition"),
    (
        r"(?i)\b(anniversary([\s\-]?edition)?)\b",
        "Anniversary Edition",
    ),
    (r"(?i)\b(redux)\b", "Redux"),
    (r"(?i)\b(open[\s\-]?matte)\b", "Open Matte"),
];

static REGEXES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    PATTERNS
        .iter()
        .map(|(p, c)| (Regex::new(p).unwrap(), *c))
        .collect()
});

/// Extract the edition.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    let norm = crate::tokens::normalize(input);

    let mut best: Option<(usize, &'static str)> = None;
    for (re, canon) in REGEXES.iter() {
        if let Some(m) = re.find(&norm) {
            let earlier = best.map(|(p, _)| m.start() < p).unwrap_or(true);
            if earlier {
                best = Some((m.start(), *canon));
            }
        }
    }
    if let Some((_, canon)) = best {
        out.edition = Some(canon.to_owned());
        out.set_confidence(ParsedField::Edition, Confidence::new(0.85));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(s: &str) -> Option<String> {
        let mut p = ParsedRelease::new(s);
        extract(s, &mut p);
        p.edition
    }

    #[test]
    fn directors_cut_variants() {
        assert_eq!(
            ed("Movie.2019.Directors.Cut.1080p.BluRay"),
            Some("Director's Cut".to_string())
        );
    }

    #[test]
    fn extended_and_imax() {
        assert_eq!(
            ed("Movie.2019.Extended.Edition.1080p"),
            Some("Extended".to_string())
        );
        assert_eq!(ed("Movie.2019.IMAX.2160p"), Some("IMAX".to_string()));
    }

    #[test]
    fn final_cut_recognized() {
        // Parity G5: cellarr previously missed "Final Cut" entirely.
        assert_eq!(
            ed("Blade.Runner.1982.The.Final.Cut.1080p.BluRay.x264-AMIABLE"),
            Some("Final Cut".to_string())
        );
    }

    #[test]
    fn none_when_absent() {
        assert_eq!(ed("Movie.2019.1080p.BluRay.x264"), None);
    }
}
