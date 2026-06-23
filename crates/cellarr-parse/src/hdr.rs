//! HDR flag extraction (HDR10/HDR10+/Dolby Vision/HLG).
//!
//! A release may advertise several HDR formats at once (e.g. a Dolby Vision +
//! HDR10 hybrid), so all matches are collected. Order is normalised (sorted) so
//! the output is deterministic regardless of token order in the title.

use std::sync::LazyLock;

use cellarr_core::parsed::{Confidence, HdrFormat, ParsedField, ParsedRelease};
use regex::Regex;

/// `(pattern, format)`. The `+`/`plus` variant is tried before bare `HDR10`.
const PATTERNS: &[(&str, HdrFormat)] = &[
    (r"(?i)\b(hdr10\+|hdr10plus|hdr\+)\b", HdrFormat::Hdr10Plus),
    (
        r"(?i)\b(dolby[\s\-]?vision|dovi|dv)\b",
        HdrFormat::DolbyVision,
    ),
    (r"(?i)\bhlg\b", HdrFormat::Hlg),
    (r"(?i)\b(hdr10|hdr)\b", HdrFormat::Hdr10),
];

static REGEXES: LazyLock<Vec<(Regex, HdrFormat)>> = LazyLock::new(|| {
    PATTERNS
        .iter()
        .map(|(p, f)| (Regex::new(p).unwrap(), *f))
        .collect()
});

/// Extract HDR flags.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    let norm = crate::tokens::normalize(input);
    let mut flags: Vec<HdrFormat> = Vec::new();
    for (re, fmt) in REGEXES.iter() {
        if re.is_match(&norm) {
            flags.push(*fmt);
        }
    }
    if flags.is_empty() {
        return;
    }
    flags.sort();
    flags.dedup();
    out.hdr = flags;
    out.set_confidence(ParsedField::Hdr, Confidence::new(0.9));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdr(s: &str) -> Vec<HdrFormat> {
        let mut p = ParsedRelease::new(s);
        extract(s, &mut p);
        p.hdr
    }

    #[test]
    fn plus_not_confused_with_base() {
        assert_eq!(
            hdr("Movie.2019.2160p.HDR10Plus.x265"),
            vec![HdrFormat::Hdr10Plus]
        );
    }

    #[test]
    fn dolby_vision() {
        let h = hdr("Movie.2019.2160p.DV.HDR10.x265");
        assert!(h.contains(&HdrFormat::DolbyVision));
        assert!(h.contains(&HdrFormat::Hdr10));
    }

    #[test]
    fn none_when_absent() {
        assert!(hdr("Show.S01E01.1080p.WEB-DL.x264").is_empty());
    }
}
