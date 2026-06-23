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
    // A full untouched disc: an explicit `BR-DISK`, a `COMPLETE.BLURAY`, or a
    // bare disc structure (`BDMV`/`M2TS`/`BD25`/`BD50`/`UHD-BD`) carried *without*
    // a remux or encode marker. Listed before the encoded-Bluray pattern so a
    // raw disc wins over the generic Bluray tier.
    (
        r"(?i)\b(br[\s\-]?disk|complete[\s\.\-]?blu[\s\-]?ray|bdmv|m2ts|bd25|bd50|uhd[\s\-]?bd)\b",
        Source::BrDisk,
    ),
    // Raw HD broadcast/transport-stream capture (no re-encode).
    (
        r"(?i)\b(raw[\s\-]?hd|hdtv[\s\-]?raw|mpeg[\s\-]?ts)\b",
        Source::RawHd,
    ),
    (
        r"(?i)\b(blu[\s\-]?ray|bluray|bdrip|brrip|hddvd|bd)\b",
        Source::Bluray,
    ),
    (r"(?i)\b(web[\s\-]?dl|webdl|web)\b", Source::WebDl),
    (r"(?i)\b(web[\s\-]?rip|webrip)\b", Source::Webrip),
    (r"(?i)\b(hdtv|hdrip)\b", Source::Hdtv),
    // DVD pre-retail tiers, most specific first (each its own quality bucket).
    (r"(?i)\b(dvdscr|dvd[\s\-]?scr|screener)\b", Source::Dvdscr),
    (r"(?i)\b(dvd[\s\-]?r|r5)\b", Source::DvdR),
    (r"(?i)\bregional\b", Source::Regional),
    (r"(?i)\b(dvdrip|dvd|dvd5|dvd9|pal|ntsc)\b", Source::Dvd),
    (r"(?i)\b(sdtv|pdtv|tvrip|dsr)\b", Source::Sdtv),
    // Cinema-source tiers, each a distinct quality bucket (Radarr keeps these
    // separate from CAM; only `cam`/`hdcam`/`camrip` are CAM-tier).
    (r"(?i)\b(workprint|wp)\b", Source::Workprint),
    (r"(?i)\b(telesync|hdts|ts)\b", Source::Telesync),
    (r"(?i)\b(telecine|tc)\b", Source::Telecine),
    (r"(?i)\b(cam|camrip|hdcam)\b", Source::Cam),
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
    // convention, but if `rip` follows it is a WEBRip. The WEBRip pattern is more
    // specific, so when both fired prefer it over the bare WEB-DL match.
    if let Some((_, src)) = chosen {
        let webdl_fired = src == Source::WebDl;
        let webrip_fired = PATTERNS
            .iter()
            .position(|(_, s)| *s == Source::Webrip)
            .is_some_and(|i| hits.matched(i));
        let final_src = if webdl_fired && webrip_fired {
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
