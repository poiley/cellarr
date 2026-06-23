//! Numbering / coordinate extraction.
//!
//! This is the hardest slice. It recognises, in priority order:
//!
//! 1. **Daily** episodes — a `YYYY MM DD` date (`Show 2019 03 14`). These are
//!    real episodes, but the date *is* the addressing; the matching
//!    season/episode is unknown without metadata, so the parser emits a
//!    [`Coordinates::Daily`] carrying the ISO `yyyy-mm-dd` date and lets Identify
//!    resolve it via the series' air-date table.
//! 2. **Season+episode** ranges — `S01E01`, `S01E01E02`, `S01E01-E03`,
//!    `1x05`, `S01.E01`.
//! 3. **Season packs** — `S01`, `Season 1`, `S01-S03`.
//! 4. **Anime absolute** — `Show - 071`, ` - 1071`, `Show 12` after a fansub
//!    bracket. The parser only *extracts* the absolute number; mapping to
//!    season/episode happens at Identify.
//!
//! Each non-episode form has its own [`Coordinates`] variant rather than an
//! `Episode` sentinel: a whole-season release is a [`Coordinates::SeasonPack`]
//! and an unmapped anime absolute is a [`Coordinates::Absolute`]. Identify
//! replaces these transient parser-stage variants with canonical
//! [`Coordinates::Episode`] addressing using the series' scene mappings.

use std::sync::LazyLock;

use cellarr_core::media::Coordinates;
use cellarr_core::parsed::{Confidence, ParsedField, ParsedRelease};
use regex::Regex;

// S01E02, s1e2, S01.E02, S01_E02 (single).
static SXXEXX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bs(\d{1,3})[\s._-]*e(\d{1,4})").unwrap());

// The whole S01E01E02E03 / S01E01-E03 multi-episode block.
static MULTI_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bs(\d{1,3})((?:[\s._-]*e\d{1,4}){2,}|[\s._-]*e\d{1,4}[\s._-]*-[\s._-]*e?\d{1,4})",
    )
    .unwrap()
});

// Every Exx inside a multi block.
static EP_IN_BLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)e(\d{1,4})").unwrap());
// A trailing range `E01-E05` or `E01-05`.
static EP_RANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)e(\d{1,4})[\s._-]*-[\s._-]*e?(\d{1,4})").unwrap());

// 1x05, 12x05 (alternate single).
static NXN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d{1,3})x(\d{1,4})\b").unwrap());

// Season pack: "Season 1", "S01" (no episode), "S01-S03".
static SEASON_RANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bs(\d{1,3})[\s._-]*-[\s._-]*s(\d{1,3})\b").unwrap());
static SEASON_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:season|series)[\s._-]*(\d{1,3})\b").unwrap());
static SEASON_SHORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bs(\d{1,3})\b").unwrap());

// Daily date YYYY MM DD (already-normalised separators).
static DAILY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(19\d{2}|20\d{2})\s(\d{2})\s(\d{2})\b").unwrap());

// Anime absolute: "- 071", "- 1071", optionally with version "- 071v2".
static ABSOLUTE_DASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-\s*(\d{1,4})(?:v\d)?\b").unwrap());
// Anime absolute after a fansub bracket: "[Group] Title 12 [1080p]".
static ABSOLUTE_BARE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\[[^\]]+\]\s+.+?\s(\d{1,4})(?:v\d)?\s*(?:\[|\(|$)").unwrap());

/// Extract numbering coordinates.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    let norm = crate::tokens::normalize(input);

    // 1. Daily date — emit a Daily coordinate carrying the ISO date.
    if let Some(c) = DAILY.captures(&norm) {
        // Guard: a date needs month 01-12 and day 01-31 to be plausible.
        let year = c.get(1).map(|m| m.as_str()).unwrap_or("");
        let month: u32 = c.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let day: u32 = c.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        if (1..=12).contains(&month) && (1..=31).contains(&day) {
            // Daily episodes are addressed by air date; the library still keys on
            // season/episode, so the date is carried as-is and Identify resolves
            // it against the series' air-date table. ISO `yyyy-mm-dd` form keeps
            // the value self-describing without a calendar dependency.
            out.coordinates.push(Coordinates::Daily {
                date: format!("{year}-{month:02}-{day:02}"),
            });
            out.set_confidence(ParsedField::Coordinates, Confidence::new(0.6));
            return;
        }
    }

    // 2. Multi-episode block.
    if let Some(block) = MULTI_BLOCK.find(&norm) {
        if let Some(sm) = SXXEXX.captures(block.as_str()) {
            let season: u32 = sm.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
            let block_str = block.as_str();
            let mut episodes: Vec<u32> = Vec::new();

            if let Some(rng) = EP_RANGE.captures(block_str) {
                let start: u32 = rng
                    .get(1)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0);
                let end: u32 = rng
                    .get(2)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0);
                if start > 0 && end >= start && end - start < 200 {
                    episodes.extend(start..=end);
                }
            } else {
                for ep in EP_IN_BLOCK.captures_iter(block_str) {
                    if let Some(n) = ep.get(1).and_then(|m| m.as_str().parse::<u32>().ok()) {
                        episodes.push(n);
                    }
                }
            }
            episodes.sort_unstable();
            episodes.dedup();
            if !episodes.is_empty() {
                for ep in episodes {
                    out.coordinates.push(Coordinates::Episode {
                        season,
                        episode: ep,
                        absolute: None,
                    });
                }
                out.set_confidence(ParsedField::Coordinates, Confidence::new(0.97));
                return;
            }
        }
    }

    // 3. Single S01E01.
    if let Some(c) = SXXEXX.captures(&norm) {
        let season: u32 = c.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let episode: u32 = c.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        out.coordinates.push(Coordinates::Episode {
            season,
            episode,
            absolute: None,
        });
        out.set_confidence(ParsedField::Coordinates, Confidence::new(0.99));
        return;
    }

    // 3b. 1x05 form.
    if let Some(c) = NXN.captures(&norm) {
        let season: u32 = c.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let episode: u32 = c.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        out.coordinates.push(Coordinates::Episode {
            season,
            episode,
            absolute: None,
        });
        out.set_confidence(ParsedField::Coordinates, Confidence::new(0.9));
        return;
    }

    // 4. Season range S01-S03 → one season-pack coordinate per covered season.
    if let Some(c) = SEASON_RANGE.captures(&norm) {
        let from: u32 = c.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let to: u32 = c.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        if from > 0 && to >= from && to - from < 100 {
            for s in from..=to {
                push_season_pack(out, s);
            }
            out.set_confidence(ParsedField::Coordinates, Confidence::new(0.9));
            return;
        }
    }

    // 4b. Season pack "Season 1" / "S01".
    let season_pack = SEASON_WORD
        .captures(&norm)
        .or_else(|| SEASON_SHORT.captures(&norm));
    if let Some(c) = season_pack {
        if let Some(s) = c.get(1).and_then(|m| m.as_str().parse::<u32>().ok()) {
            push_season_pack(out, s);
            out.set_confidence(ParsedField::Coordinates, Confidence::new(0.85));
            return;
        }
    }

    // 5. Anime absolute. Try the fansub-bracket bare form first (most reliable),
    //    then the dash form. Identify remaps the absolute to a real episode.
    if let Some(c) = ABSOLUTE_BARE.captures(&norm) {
        if let Some(n) = c.get(1).and_then(|m| m.as_str().parse::<u32>().ok()) {
            if n > 0 {
                push_absolute(out, n, 0.8);
                return;
            }
        }
    }
    if let Some(c) = ABSOLUTE_DASH.captures(&norm) {
        if let Some(n) = c.get(1).and_then(|m| m.as_str().parse::<u32>().ok()) {
            if n > 0 && n < 5000 {
                push_absolute(out, n, 0.7);
            }
        }
    }
}

fn push_season_pack(out: &mut ParsedRelease, season: u32) {
    // Coordinates::SeasonPack carries a u16 season; release seasons never
    // approach that ceiling, but clamp defensively rather than wrap.
    out.coordinates.push(Coordinates::SeasonPack {
        season: u16::try_from(season).unwrap_or(u16::MAX),
    });
}

fn push_absolute(out: &mut ParsedRelease, absolute: u32, conf: f32) {
    out.coordinates
        .push(Coordinates::Absolute { number: absolute });
    out.set_confidence(ParsedField::Coordinates, Confidence::new(conf));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coords(s: &str) -> Vec<Coordinates> {
        let mut p = ParsedRelease::new(s);
        extract(s, &mut p);
        p.coordinates
    }

    #[test]
    fn single_episode() {
        assert_eq!(
            coords("Show.S02E15.1080p.BluRay.x264-GRP"),
            vec![Coordinates::Episode {
                season: 2,
                episode: 15,
                absolute: None
            }]
        );
    }

    #[test]
    fn multi_episode_consecutive() {
        assert_eq!(
            coords("Show.S01E01E02E03.720p.HDTV"),
            vec![
                Coordinates::Episode {
                    season: 1,
                    episode: 1,
                    absolute: None
                },
                Coordinates::Episode {
                    season: 1,
                    episode: 2,
                    absolute: None
                },
                Coordinates::Episode {
                    season: 1,
                    episode: 3,
                    absolute: None
                },
            ]
        );
    }

    #[test]
    fn multi_episode_range() {
        assert_eq!(
            coords("Show.S01E01-E04.720p.HDTV"),
            vec![
                Coordinates::Episode {
                    season: 1,
                    episode: 1,
                    absolute: None
                },
                Coordinates::Episode {
                    season: 1,
                    episode: 2,
                    absolute: None
                },
                Coordinates::Episode {
                    season: 1,
                    episode: 3,
                    absolute: None
                },
                Coordinates::Episode {
                    season: 1,
                    episode: 4,
                    absolute: None
                },
            ]
        );
    }

    #[test]
    fn alt_form_1x05() {
        assert_eq!(
            coords("Show 1x05 720p HDTV"),
            vec![Coordinates::Episode {
                season: 1,
                episode: 5,
                absolute: None
            }]
        );
    }

    #[test]
    fn season_pack() {
        assert_eq!(
            coords("Show.S03.1080p.WEB-DL.x264-GRP"),
            vec![Coordinates::SeasonPack { season: 3 }]
        );
        assert_eq!(
            coords("Show.Season.2.1080p.WEB-DL"),
            vec![Coordinates::SeasonPack { season: 2 }]
        );
    }

    #[test]
    fn season_range_pack_per_season() {
        assert_eq!(
            coords("Show.S01-S03.1080p.WEB-DL.x264-GRP"),
            vec![
                Coordinates::SeasonPack { season: 1 },
                Coordinates::SeasonPack { season: 2 },
                Coordinates::SeasonPack { season: 3 },
            ]
        );
    }

    #[test]
    fn anime_absolute_dash() {
        assert_eq!(
            coords("[SubsPlease] Show - 1071 (1080p) [ABCD1234].mkv"),
            vec![Coordinates::Absolute { number: 1071 }]
        );
    }

    #[test]
    fn daily_emits_iso_date() {
        // Daily episode: surfaced as a date coordinate, not season/episode.
        assert_eq!(
            coords("Show.2019.03.14.1080p.WEB-DL.x264-GRP"),
            vec![Coordinates::Daily {
                date: "2019-03-14".to_string()
            }]
        );
    }
}
