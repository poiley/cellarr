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
    // The group is the last dash-delimited token; a trailing media extension
    // (`.mkv`, `.mp4`) is stripped first by `extract`, so the group itself never
    // contains a `.`.
    Regex::new(r"(?x) - ([A-Za-z0-9][A-Za-z0-9_@]{1,24}) \s* $").unwrap()
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
    fn none_when_absent() {
        assert_eq!(group("Show.S01E01.1080p.WEBDL.x264"), None);
    }
}
