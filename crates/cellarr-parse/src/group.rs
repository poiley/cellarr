//! Release-group extraction.
//!
//! Scene/p2p convention puts the group after the final `-` (e.g.
//! `...x264-GROUP`). Anime fansubs instead lead with a bracketed group
//! (`[HorribleSubs] Show - 01`). We try the trailing-dash form first, then the
//! leading-bracket form, and reject obvious non-groups (a bare quality token, a
//! file extension, or a `[...]` CRC hash).

use std::sync::LazyLock;

use cellarr_core::parsed::{Confidence, ParsedField, ParsedRelease};
use regex::Regex;

// Trailing `-GROUP` at the very end (optionally before a file extension or a
// trailing bracket tag). The group is letters/digits/limited punctuation and no
// spaces.
static TRAILING: LazyLock<Regex> = LazyLock::new(|| {
    // The group follows the final dash that begins a run reaching the end. The
    // inner class allows hyphens so hyphenated groups (e.g. `D-Z0N3`) are captured
    // whole; since scene names are dot-separated and the class excludes `.`, the
    // match is bounded to the trailing token. A trailing media extension
    // (`.mkv`, …) is stripped first by `extract`, so the group never contains `.`.
    Regex::new(r"(?x) - ([A-Za-z0-9][A-Za-z0-9_@-]{1,24}) \s* $").unwrap()
});

// Common media container/subtitle extensions to strip before group matching.
static EXTENSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\.(mkv|mp4|avi|mov|m4v|ts|wmv|flv|webm|srt|ass|sub|idx)$").unwrap()
});

// Leading `[Group]` at the very start (anime fansub convention).
static LEADING_BRACKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\[([^\]]{2,30})\]").unwrap());

// An 8-hex-digit CRC in brackets is not a group.
static CRC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[0-9a-f]{8}$").unwrap());

// Tokens that are quality/format words, never a group, even after a dash.
const NON_GROUP: &[&str] = &[
    "x264", "x265", "h264", "h265", "hevc", "avc", "av1", "1080p", "720p", "2160p", "480p", "576p",
    "web", "webdl", "webrip", "bluray", "hdtv", "remux", "proper", "repack", "dts", "ac3", "aac",
    "hdr", "hdr10", "dv", "atmos", "truehd", "xvid", "divx", "internal", "dl", "rip", "ray", "ma",
    "hd", "dts", "e", "dual", "subs",
];

/// Extract the release group.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    // Operate on the raw input (not normalised) so dashes and brackets survive.
    let trimmed = EXTENSION.replace(input.trim(), "");
    let trimmed = trimmed.as_ref();

    if let Some(c) = TRAILING.captures(trimmed) {
        if let Some(g) = c.get(1) {
            let cand = g.as_str().trim_matches('.');
            // Allowing internal hyphens (for groups like `D-Z0N3`) can over-capture
            // when a hyphenated source tag precedes the group (`WEB-DL-GRP` →
            // `DL-GRP`). Drop leading hyphen-segments that are known non-group
            // tokens so `WEB-DL-GRP` yields `GRP` while `D-Z0N3` stays intact.
            let cand = strip_leading_non_group_segments(cand);
            if is_plausible_group(cand) {
                out.group = Some(cand.to_owned());
                out.set_confidence(ParsedField::Group, Confidence::new(0.9));
                return;
            }
        }
    }

    if let Some(c) = LEADING_BRACKET.captures(trimmed) {
        if let Some(g) = c.get(1) {
            let cand = g.as_str().trim();
            if is_plausible_group(cand) && !CRC.is_match(cand) {
                out.group = Some(cand.to_owned());
                out.set_confidence(ParsedField::Group, Confidence::new(0.8));
            }
        }
    }
}

/// Drop leading hyphen-delimited segments that are known non-group tokens
/// (source/codec words), so a hyphenated source tag bleeding into the capture
/// (`WEB-DL-GRP`) is reduced to the real group (`GRP`). A hyphenated group whose
/// first segment is not a known token (`D-Z0N3`) is returned unchanged.
fn strip_leading_non_group_segments(cand: &str) -> &str {
    let mut start = 0usize;
    loop {
        let rest = &cand[start..];
        let Some(dash) = rest.find('-') else {
            break; // last (or only) segment — keep it even if it's a token
        };
        let seg = &rest[..dash];
        if NON_GROUP.contains(&seg.to_ascii_lowercase().as_str()) {
            start += dash + 1; // skip this segment and its trailing '-'
        } else {
            break;
        }
    }
    &cand[start..]
}

fn is_plausible_group(cand: &str) -> bool {
    if cand.len() < 2 || cand.len() > 30 {
        return false;
    }
    let lower = cand.to_ascii_lowercase();
    if NON_GROUP.contains(&lower.as_str()) {
        return false;
    }
    // Must contain at least one ASCII letter (purely numeric is not a group).
    cand.chars().any(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(s: &str) -> Option<String> {
        let mut p = ParsedRelease::new(s);
        extract(s, &mut p);
        p.group
    }

    #[test]
    fn trailing_dash_group() {
        assert_eq!(
            group("Show.S01E01.1080p.WEB-DL.x264-GROUP"),
            Some("GROUP".to_string())
        );
        assert_eq!(
            group("Movie.2019.1080p.BluRay.x264-SPARKS"),
            Some("SPARKS".to_string())
        );
        // Hyphenated group captured whole (G6).
        assert_eq!(
            group("Movie.2019.1080p.BluRay.x264-D-Z0N3"),
            Some("D-Z0N3".to_string())
        );
    }

    #[test]
    fn leading_bracket_fansub() {
        assert_eq!(
            group("[HorribleSubs] Show - 01 [1080p].mkv"),
            Some("HorribleSubs".to_string())
        );
    }

    #[test]
    fn quality_token_after_dash_is_not_group() {
        // `WEB-DL` ends in `DL` after a dash but is not a group.
        assert_eq!(group("Show.S01E01.1080p.WEB-DL"), None);
    }

    #[test]
    fn hyphenated_source_tag_does_not_bleed_into_group() {
        // `WEB-DL-GRP`: the source tag's hyphen must not capture into the group.
        assert_eq!(
            group("Show.S01E01.PROPER.REPACK.1080p.WEB-DL-GRP"),
            Some("GRP".to_string())
        );
    }

    #[test]
    fn none_when_absent() {
        assert_eq!(group("Show.S01E01.1080p.WEBDL.x264"), None);
    }
}
