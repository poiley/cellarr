//! Resolution extraction (480p/576p/720p/1080p/2160p and common aliases).

use std::sync::LazyLock;

use cellarr_core::parsed::{Confidence, ParsedField, ParsedRelease, Resolution};
use regex::Regex;

// `4k`/`uhd` map to 2160p; `2160`, `1080`, `720`, `576`, `480` may appear with or
// without the trailing `p`/`i`. The literal numbers are anchored on word
// boundaries so a year like `1080`-prefixed token is not mistaken (years are a
// 19xx/20xx range, handled separately). The `p`-suffixed forms additionally allow
// a glued letter prefix (`BD720p`, `Bluray1080p`) — common when a source tag and
// the resolution run together — by not requiring a leading boundary there.
static RES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \b( 2160p? | 4k | uhd | 1080p? | 1080i | 720p? | 576p? | 576i | 480p? | 480i )\b
        | ( 2160 | 1080 | 720 | 576 | 540 | 480 ) p          # glued: BD720p, 540p
        ",
    )
    .unwrap()
});

// Explicit `WIDTHxHEIGHT` dimensions (anime convention: `1920x1080`, `1280x720`,
// `640x480`). Mapped to the nearest standard tier by height.
static DIMENSIONS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(\d{3,4})\s*x\s*(\d{3,4})\b").unwrap());

fn tier_from_token(tok: &str) -> Option<Resolution> {
    match tok.trim_end_matches(['p', 'i']) {
        "2160" | "4k" | "uhd" => Some(Resolution::R2160p),
        "1080" => Some(Resolution::R1080p),
        "720" => Some(Resolution::R720p),
        "576" => Some(Resolution::R576p),
        // 540p is binned to the 480p (SD) tier by upstream.
        "480" | "540" => Some(Resolution::R480p),
        _ => None,
    }
}

// Map a pixel height to the nearest standard tier (upstream's WxH binning).
fn tier_from_height(h: u32) -> Option<Resolution> {
    Some(match h {
        0..=540 => Resolution::R480p,
        541..=620 => Resolution::R576p,
        621..=899 => Resolution::R720p,
        900..=1559 => Resolution::R1080p,
        _ => Resolution::R2160p,
    })
}

/// Extract the video resolution.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    let norm = crate::tokens::normalize(input);

    if let Some(m) = RES.find(&norm) {
        let tok = m.as_str().to_ascii_lowercase();
        if let Some(res) = tier_from_token(&tok) {
            out.resolution = Some(res);
            // `4k`/`uhd`/interlaced spellings are slightly less certain than the
            // explicit `1080p`-style tag, but all are strong signals.
            let conf = if tok.ends_with('p') { 1.0 } else { 0.9 };
            out.set_confidence(ParsedField::Resolution, Confidence::new(conf));
            return;
        }
    }

    // Fallback: explicit pixel dimensions (`1280x720`).
    if let Some(c) = DIMENSIONS.captures(&norm) {
        if let Some(h) = c.get(2).and_then(|m| m.as_str().parse::<u32>().ok()) {
            if let Some(res) = tier_from_height(h) {
                out.resolution = Some(res);
                out.set_confidence(ParsedField::Resolution, Confidence::new(0.85));
            }
        }
    }
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
