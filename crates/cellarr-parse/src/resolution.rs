//! Resolution extraction (480p/576p/720p/1080p/2160p and common aliases).

use std::sync::LazyLock;

use cellarr_core::parsed::{Confidence, ParsedField, ParsedRelease, Resolution};
use regex::Regex;

// `4k`/`uhd` map to 2160p; `2160`, `1080`, `720`, `576`, `480` may appear with or
// without the trailing `p`/`i`. The literal numbers are anchored on word
// boundaries so a year like `1080`-prefixed token is not mistaken (years are a
// 19xx/20xx range, handled separately).
static RES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \b(
            2160p? | 4k | uhd |
            1080p? | 1080i |
            720p? |
            576p? | 576i |
            480p? | 480i
        )\b",
    )
    .unwrap()
});

/// Extract the video resolution.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    let norm = crate::tokens::normalize(input);
    let Some(m) = RES.find(&norm) else {
        return;
    };
    let tok = m.as_str().to_ascii_lowercase();
    let res = match tok.as_str() {
        "2160p" | "2160" | "4k" | "uhd" => Resolution::R2160p,
        "1080p" | "1080" | "1080i" => Resolution::R1080p,
        "720p" | "720" => Resolution::R720p,
        "576p" | "576" | "576i" => Resolution::R576p,
        "480p" | "480" | "480i" => Resolution::R480p,
        _ => return,
    };
    out.resolution = Some(res);
    // `4k`/`uhd`/interlaced spellings are slightly less certain than the explicit
    // `1080p`-style tag, but all are strong signals.
    let conf = if tok.ends_with('p') { 1.0 } else { 0.9 };
    out.set_confidence(ParsedField::Resolution, Confidence::new(conf));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(s: &str) -> Option<Resolution> {
        let mut p = ParsedRelease::new(s);
        extract(s, &mut p);
        p.resolution
    }

    #[test]
    fn explicit_p_forms() {
        assert_eq!(res("Show.S01E01.1080p.WEB-DL"), Some(Resolution::R1080p));
        assert_eq!(res("Show.S01E01.720p.HDTV"), Some(Resolution::R720p));
        assert_eq!(res("Movie.2019.2160p.BluRay"), Some(Resolution::R2160p));
    }

    #[test]
    fn aliases() {
        assert_eq!(res("Movie.2019.4K.UHD.BluRay"), Some(Resolution::R2160p));
        assert_eq!(res("Movie.2019.UHD.BluRay"), Some(Resolution::R2160p));
    }

    #[test]
    fn none_when_absent() {
        assert_eq!(res("Show.S01E01.WEB-DL-GRP"), None);
    }

    #[test]
    fn does_not_match_x264() {
        // `264` must not be read as a resolution.
        assert_eq!(res("Show.S01E01.x264-GRP"), None);
    }
}
