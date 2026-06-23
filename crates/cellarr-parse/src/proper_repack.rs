//! Proper/Repack flag extraction.
//!
//! `REPACK` (and the rarer `RERIP`) is a re-upload fix; `PROPER` re-releases a
//! flawed earlier release. When both somehow appear, `REPACK` is reported since
//! it is the more specific marker.

use std::sync::LazyLock;

use cellarr_core::parsed::{Confidence, ParsedField, ParsedRelease, ProperRepack};
use regex::Regex;

static REPACK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(repack\d?|rerip)\b").unwrap());
static PROPER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bproper\b").unwrap());

/// Extract the proper/repack marker.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    let norm = crate::tokens::normalize(input);
    let marker = if REPACK.is_match(&norm) {
        Some(ProperRepack::Repack)
    } else if PROPER.is_match(&norm) {
        Some(ProperRepack::Proper)
    } else {
        None
    };
    if let Some(m) = marker {
        out.proper_repack = Some(m);
        out.set_confidence(ParsedField::ProperRepack, Confidence::new(0.95));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(s: &str) -> Option<ProperRepack> {
        let mut p = ParsedRelease::new(s);
        extract(s, &mut p);
        p.proper_repack
    }

    #[test]
    fn proper_and_repack() {
        assert_eq!(
            pr("Show.S01E01.PROPER.1080p.WEB-DL"),
            Some(ProperRepack::Proper)
        );
        assert_eq!(
            pr("Show.S01E01.REPACK.1080p.WEB-DL"),
            Some(ProperRepack::Repack)
        );
    }

    #[test]
    fn repack_wins_over_proper() {
        assert_eq!(
            pr("Show.S01E01.PROPER.REPACK.1080p"),
            Some(ProperRepack::Repack)
        );
    }

    #[test]
    fn none_when_absent() {
        assert_eq!(pr("Show.S01E01.1080p.WEB-DL"), None);
    }
}
