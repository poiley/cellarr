//! Source/medium extraction (Remux/BluRay/WEB-DL/WEBRip/HDTV/DVD/SDTV/Cam).
//!
//! Several source spellings overlap (e.g. `WEB-DL`, `WEBDL`, `WEB`), so this
//! phase evaluates many patterns in a single linear pass with a [`RegexSet`] and
//! then resolves ambiguity by a fixed precedence: a Remux is more specific than
//! a BluRay, a WEB-DL more specific than a bare WEB, etc.

use std::sync::LazyLock;

use cellarr_core::parsed::{Confidence, ParsedField, ParsedRelease, Source};
use regex::RegexSet;

/// Source patterns, highest specificity first. The index into this slice is the
/// precedence: an earlier match wins over a later one.
const PATTERNS: &[(&str, Source)] = &[
    (r"(?i)\bremux\b", Source::Remux),
    (r"(?i)\bbd[\s\-]?remux\b", Source::Remux),
    (
        r"(?i)\b(blu[\s\-]?ray|bluray|bdrip|brrip|bd25|bd50|bdmv|hddvd|bd)\b",
        Source::Bluray,
    ),
    (r"(?i)\b(web[\s\-]?dl|webdl|web)\b", Source::WebDl),
    (r"(?i)\b(web[\s\-]?rip|webrip)\b", Source::Webrip),
    (r"(?i)\b(hdtv|hdrip)\b", Source::Hdtv),
    (r"(?i)\b(dvdrip|dvd|dvd5|dvd9|pal|ntsc)\b", Source::Dvd),
    (r"(?i)\b(sdtv|pdtv|tvrip|dsr)\b", Source::Sdtv),
    (
        r"(?i)\b(cam|camrip|ts|telesync|tc|telecine|hdcam|hdts)\b",
        Source::Cam,
    ),
];

static SET: LazyLock<RegexSet> =
    LazyLock::new(|| RegexSet::new(PATTERNS.iter().map(|(p, _)| *p)).unwrap());

/// Extract the source/medium.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    let norm = crate::tokens::normalize(input);
    let hits = SET.matches(&norm);

    // `WEB-DL` and `WEBRip` both contain `web`; the WebDl pattern also matches a
    // bare `WEB`. Disambiguate: if the more specific WEBRip pattern fired, prefer
    // it over the WEB-DL pattern.
    let mut chosen: Option<(usize, Source)> = None;
    for (idx, (_, src)) in PATTERNS.iter().enumerate() {
        if hits.matched(idx) {
            chosen = Some((idx, *src));
            break;
        }
    }

    // Refine the WEB family: a bare `WEB` token alone is WEB-DL by scene
    // convention, but if `rip` follows it is a WEBRip.
    if let Some((idx, src)) = chosen {
        let webdl_idx = 3;
        let webrip_idx = 4;
        let final_src = if idx == webdl_idx && hits.matched(webrip_idx) {
            Source::Webrip
        } else {
            src
        };
        out.source = Some(final_src);
        out.set_confidence(ParsedField::Source, Confidence::new(0.95));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(s: &str) -> Option<Source> {
        let mut p = ParsedRelease::new(s);
        extract(s, &mut p);
        p.source
    }

    #[test]
    fn web_family() {
        assert_eq!(src("Show.S01E01.1080p.WEB-DL.x264"), Some(Source::WebDl));
        assert_eq!(src("Show.S01E01.1080p.WEBRip.x264"), Some(Source::Webrip));
        assert_eq!(src("Show.S01E01.1080p.WEB.x264"), Some(Source::WebDl));
    }

    #[test]
    fn bluray_and_remux() {
        assert_eq!(src("Movie.2019.1080p.BluRay.x264"), Some(Source::Bluray));
        assert_eq!(
            src("Movie.2019.2160p.BluRay.REMUX.HEVC"),
            Some(Source::Remux)
        );
        assert_eq!(src("Movie.2019.720p.BRRip.x264"), Some(Source::Bluray));
    }

    #[test]
    fn broadcast_and_disc() {
        assert_eq!(src("Show.S01E01.HDTV.x264"), Some(Source::Hdtv));
        assert_eq!(src("Movie.2001.DVDRip.XviD"), Some(Source::Dvd));
    }
}
