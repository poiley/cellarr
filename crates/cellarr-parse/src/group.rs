//! Release-group extraction.
//!
//! Scene/p2p convention puts the group after the final `-` (e.g.
//! `...x264-GROUP`). Anime fansubs instead lead with a bracketed group
//! (`[HorribleSubs] Show - 01`). p2p x265 encoders trail the group inside the
//! quality parens (`(... 10bit AAC 7.1 Tigole)`) or in a final bracket
//! (`[YTS.MX]`, `[QxR]`). We try these forms in order and reject obvious
//! non-groups (a bare quality token, a file extension, or a `[...]` CRC hash).
//!
//! We also strip the family of **repost/obfuscation suffixes** that scene
//! re-uploaders append after the real group (`EVO-Rakuv`, `NTb-postbot`,
//! `DON-Obfuscated`); upstream Sonarr/Radarr peel these off so the underlying
//! group is what matches. The suffix set below is a re-curated *fact list* of
//! those known tags (clean-room — facts, not code).

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

// Trailing `GROUP` that follows a final `-` or `.` and reaches the end. Used as a
// fallback so a dot-separated group (`...x264.D-Z0N3`, `...MA.5.1.KRaLiMaRKo`) is
// captured whole rather than truncated at an internal hyphen.
static TRAILING_DOT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?x) [-.] ([A-Za-z][A-Za-z0-9_-]{1,24}) \s* $").unwrap());

// Common media container/subtitle extensions to strip before group matching.
static EXTENSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\.(mkv|mp4|avi|mov|m4v|ts|wmv|flv|webm|srt|ass|sub|idx)$").unwrap()
});

// One or more trailing site/source tags in brackets after the real group token,
// e.g. `-2HD [eztv]-[rarbg.com]` or `-ROUGH [PublicHD]`. These are stripped so the
// trailing-dash matcher sees the group as the final token.
static TRAILING_SITE_TAGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?ix) \s* (?: [-\s]* \[ [^\]]{1,20} \] )+ \s* $").unwrap());

// A final `[GROUP]` (optionally after a `-`), the p2p/movie convention
// (`x264-[YTS.MX]`, `...Subs][HDO]`, `[QxR]`). Captured only when the bracket
// content looks like a group (letters, not a pure CRC / quality token).
static FINAL_BRACKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?x) \[ ([A-Za-z0-9][A-Za-z0-9_.\-]{1,20}) \] \s* $").unwrap());

// A group trailing inside the final parens after quality tokens, the x265
// encoder convention: `(1080p BluRay x265 10bit AAC 7.1 Tigole)`. We take the
// last whitespace token before the closing paren when an encode marker
// (`x265`/`x264`/`hevc`/`h265`) appears inside that paren group.
static FINAL_PAREN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?x) \( ([^()]*) \) \s* (?: \[ [A-Za-z0-9_.\-]{1,20} \] \s* )? $").unwrap()
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
    "hd", "dts", "e", "dual", "subs", "10bit", "8bit", "sdr", "ddp", "eac3", "flac", "vc",
];

// Re-curated fact list: repost / obfuscation / "as-requested" suffixes that
// scene re-uploaders append after the real group with a hyphen. Upstream peels
// these off (matched case-insensitively, exact segment). Some carry a numeric
// tail (`Rakuv02`), handled by a prefix check below.
const REPOST_SUFFIXES: &[&str] = &[
    "rakuv",
    "rakuvfinhel",
    "rakuvus",
    "obfuscated",
    "obfuscation",
    "scrambled",
    "postbot",
    "xpost",
    "asrequested",
    "buymore",
    "chamele0n",
    "gerov",
    "z0ids3n",
    "nzbgeek",
    "alteZachen",
    "alteachen",
    "altezachen",
    "whiterev",
    "4p",
    "4planet",
    "rp",
    "rp-rp",
    "pre",
    "sample",
    "repackpost",
    "1",
];

/// Extract the release group.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    // Operate on the raw input (not normalised) so dashes and brackets survive.
    let trimmed = EXTENSION.replace(input.trim(), "");
    let trimmed = trimmed.as_ref();

    // Trailing `-GROUP` (the dominant scene form). Strip trailing site tags first
    // so `-2HD [eztv]-[rarbg.com]` resolves to `2HD`.
    let detagged = TRAILING_SITE_TAGS.replace(trimmed, "");
    if let Some(g) = match_trailing_dash(detagged.as_ref()) {
        finalize(out, g, 0.9);
        return;
    }

    // Final `[GROUP]` bracket (`x264-[YTS.MX]`, `...Subs][HDO]`).
    if let Some(c) = FINAL_BRACKET.captures(trimmed) {
        if let Some(g) = c.get(1) {
            let cand = g.as_str().trim_matches(['.', '-']);
            if is_plausible_group(cand) && !CRC.is_match(cand) {
                finalize(out, strip_repost(cand).to_owned(), 0.75);
                return;
            }
        }
    }

    // Group trailing inside the quality parens (x265 p2p convention).
    if let Some(g) = match_final_paren(trimmed) {
        finalize(out, g, 0.7);
        return;
    }

    // Dot-separated trailing group (`...x264.D-Z0N3`, `...MA.5.1.KRaLiMaRKo`).
    if let Some(c) = TRAILING_DOT.captures(trimmed) {
        if let Some(g) = c.get(1) {
            // Reuse the source-tag bleed guard so `.WEB-DL` (→ `DL`, a non-group)
            // is rejected while `.D-Z0N3` survives.
            let cand = strip_leading_non_group_segments(g.as_str());
            let cand = strip_repost(cand);
            if is_plausible_group(cand) {
                finalize(out, cand.to_owned(), 0.7);
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

/// Match the trailing `-GROUP`, applying the bleed guard and repost-suffix strip.
fn match_trailing_dash(s: &str) -> Option<String> {
    let c = TRAILING.captures(s)?;
    let cand = c.get(1)?.as_str().trim_matches('.');
    // Allowing internal hyphens (for groups like `D-Z0N3`) can over-capture when a
    // hyphenated source tag precedes the group (`WEB-DL-GRP` → `DL-GRP`). Drop
    // leading hyphen-segments that are known non-group tokens.
    let cand = strip_leading_non_group_segments(cand);
    // Peel a trailing repost/obfuscation suffix (`EVO-Rakuv` → `EVO`).
    let cand = strip_repost(cand);
    if is_plausible_group(cand) {
        Some(cand.to_owned())
    } else {
        None
    }
}

/// Take the last token inside the final parens when that paren block carries an
/// encode marker (so it is the quality+group paren, not a stray descriptor).
fn match_final_paren(s: &str) -> Option<String> {
    let c = FINAL_PAREN.captures(s)?;
    let inner = c.get(1)?.as_str().trim();
    let lower = inner.to_ascii_lowercase();
    let has_encode = ["x265", "x264", "h265", "h264", "hevc"]
        .iter()
        .any(|m| lower.split_whitespace().any(|t| t == *m));
    if !has_encode {
        return None;
    }
    let last = inner.split_whitespace().last()?;
    let cand = last.trim_matches(['.', '-']);
    if is_plausible_group(cand) && !cand.chars().all(|c| c.is_ascii_digit()) {
        Some(cand.to_owned())
    } else {
        None
    }
}

fn finalize(out: &mut ParsedRelease, group: String, conf: f32) {
    out.group = Some(group);
    out.set_confidence(ParsedField::Group, Confidence::new(conf));
}

/// If the candidate ends with `-<known repost suffix>` (one or more), peel them
/// off, returning the underlying group. `EVO-Rakuv-RP` → `EVO`.
fn strip_repost(cand: &str) -> &str {
    let mut cur = cand;
    while let Some(dash) = cur.rfind('-') {
        let (head, tail) = (&cur[..dash], &cur[dash + 1..]);
        if head.is_empty() {
            break;
        }
        let tl = tail.to_ascii_lowercase();
        let is_repost = REPOST_SUFFIXES.contains(&tl.as_str())
            // Numeric-tailed variants: `Rakuv02`, `RP2`.
            || REPOST_SUFFIXES.iter().any(|s| {
                tl.starts_with(s)
                    && tl[s.len()..].chars().all(|c| c.is_ascii_digit())
                    && tl.len() > s.len()
            });
        if is_repost {
            cur = head;
        } else {
            break;
        }
    }
    cur
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
