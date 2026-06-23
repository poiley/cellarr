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

// The whole multi-episode block following a single `Sxx` marker:
// `S01E01E02E03`, `S01E01-E03`, `S01E01-02-03`, `S6.E1-E2-E3`, `S6E1-S6E2`.
// Allows repeated `S<same>` markers and bare `Eyy` / `-yy` continuations.
static MULTI_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \b s\d{1,3} [\s._-]* e \d{1,4}       # the first SxxEyy
        (                                    # continuation: 1+ more episodes, each
                                             # carrying an explicit `E`/`S..E` marker
                                             # OR joined by a hyphen (range/list)
            (?:
                [\s._-]* (?:s\d{1,3}[\s._-]*)? e \d{1,4}   # SxxEzz / Ezz
              | [\s._-]*-[\s._-]* e? \d{1,4}               # -zz / -Ezz
            )+
        )",
    )
    .unwrap()
});

// Every episode number inside a multi block (the digits after an optional
// `S<n>` / `E` / separator). Skips a season repeat so `S6E1-S6E2` yields [1,2].
static EP_IN_BLOCK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:s\d{1,3}[\s._-]*)?e?(\d{1,4})").unwrap());

// 1x05, 12x05 (alternate single). Also matches the multi forms `2x04x05`,
// `2x04.2x05`, `2x01-x02` via `nxn_episodes`.
static NXN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d{1,3})x(\d{1,4})\b").unwrap());

// A multi NxN block: `2x04x05`, `2x04.2x05`, `2x01-x02`, `2x9-2x10`, `1x01-x03`.
static NXN_MULTI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \b (\d{1,3}) x \d{1,4}
        (?:                                  # continuation, each carrying an `x`
                                             # marker OR a hyphen (range/list)
            (?: [\s._-]* (?:\d{1,3})? x \d{1,4} )
          | (?: [\s._-]*-[\s._-]* \d{1,4} )
        )+",
    )
    .unwrap()
});
// Each episode number inside an NxN multi block (`x05`, `2x05`, bare `-05`).
static NXN_EP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:\d{1,3}x|x|-)(\d{1,4})").unwrap());

// `Season 1 Episode 5-6`, `Ep10718 - Ep10722` (word-form episode range).
static SEASON_EP_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \b season [\s._-]* (\d{1,3}) [\s._-]* episode [\s._-]* (\d{1,4}) [\s._-]*-[\s._-]* (\d{1,4})",
    )
    .unwrap()
});
static EP_PREFIX_RANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?ix) \b s(\d{1,3}) [\s._-]* ep (\d{1,5}) [\s._-]*-[\s._-]* ep (\d{1,5})").unwrap()
});

// Season pack: "Season 1", "S01" (no episode), "S01-S03".
static SEASON_RANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bs(\d{1,3})[\s._-]*-[\s._-]*s(\d{1,3})\b").unwrap());
// Season-pack words, incl. foreign-language spellings (Saison/Stagione/Temporada).
static SEASON_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:season|series|saison|stagione|temporada|seizoen|sezon|staffel)[\s._-]*(\d{1,4})\b",
    )
    .unwrap()
});
// A spaced-out `S 01` marker (no episode).
static SEASON_SPACED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bs[\s_.-]+(\d{1,4})\b").unwrap());
static SEASON_SHORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bs(\d{1,4})\b").unwrap());

// Daily date YYYY MM DD. `normalize` turns `.`/`_`/space into a single space but
// leaves `-`, so accept either a space or a hyphen between the parts to also catch
// the ISO `YYYY-MM-DD` form (`Series - 2013-10-30 - Episode …`).
static DAILY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(19\d{2}|20\d{2})[\s-](\d{2})[\s-](\d{2})\b").unwrap());

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

    // 2a. Word-form `Season N Episode A-B` and `Sxx EpA - EpB`.
    if let Some(c) = SEASON_EP_WORD.captures(&norm).or_else(|| {
        EP_PREFIX_RANGE
            .captures(&norm)
            .filter(|_| EP_PREFIX_RANGE.is_match(&norm))
    }) {
        let season: u32 = c.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let a: u32 = c.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let b: u32 = c.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        if a > 0 && b >= a && b - a < 500 {
            push_episodes(out, season, (a..=b).collect(), 0.95);
            return;
        }
    }

    // 2b. Multi-episode block following an `Sxx` marker.
    if let Some(block) = MULTI_BLOCK.find(&norm) {
        if let Some(sm) = SXXEXX.captures(block.as_str()) {
            let season: u32 = sm.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
            let nums: Vec<u32> = EP_IN_BLOCK
                .captures_iter(block.as_str())
                .filter_map(|c| c.get(1).and_then(|m| m.as_str().parse::<u32>().ok()))
                .collect();
            let episodes = expand_episode_list(&nums);
            if !episodes.is_empty() {
                push_episodes(out, season, episodes, 0.97);
                return;
            }
        }
    }

    // 2c. Multi NxN block (`2x04x05`, `2x04.2x05`, `2x01-x02`, `1x01-x03`).
    if let Some(block) = NXN_MULTI.find(&norm) {
        if let Some(sm) = NXN.captures(block.as_str()) {
            let season: u32 = sm.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
            let nums: Vec<u32> = NXN_EP
                .captures_iter(block.as_str())
                .filter_map(|c| c.get(1).and_then(|m| m.as_str().parse::<u32>().ok()))
                .collect();
            let episodes = expand_episode_list(&nums);
            if episodes.len() >= 2 {
                push_episodes(out, season, episodes, 0.9);
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

    // 4b. Season pack "Season 1" / "Saison 3" / "S 01" / "S01".
    let season_pack = SEASON_WORD
        .captures(&norm)
        .or_else(|| SEASON_SPACED.captures(&norm))
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

/// Expand a list of episode numbers parsed from a multi-block into the final
/// episode set. An exactly-two-number block is treated as an inclusive range
/// (`E01-E03` → 1,2,3; `E22-E23` → 22,23); three or more numbers are an explicit
/// list (`E01-02-03` → 1,2,3; `E96-97-98-99-100` verbatim). Defends against a
/// runaway range.
fn expand_episode_list(nums: &[u32]) -> Vec<u32> {
    let mut episodes: Vec<u32> = match nums {
        [a, b] if *b >= *a && *b - *a < 200 => (*a..=*b).collect(),
        _ => nums.to_vec(),
    };
    episodes.sort_unstable();
    episodes.dedup();
    episodes.retain(|n| *n > 0);
    episodes
}

fn push_episodes(out: &mut ParsedRelease, season: u32, episodes: Vec<u32>, conf: f32) {
    for ep in episodes {
        out.coordinates.push(Coordinates::Episode {
            season,
            episode: ep,
            absolute: None,
        });
    }
    out.set_confidence(ParsedField::Coordinates, Confidence::new(conf));
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

    // --- guard-boundary tests (pin operators a mutation run found unguarded) ---

    /// A `YYYY NN NN` triple is a Daily date only when BOTH the month is 01-12
    /// AND the day is 01-31. A plausible-looking but invalid date (month 13, or
    /// day 32) must NOT be treated as a daily episode — it falls through so the
    /// digits can be parsed by a later rule (or left unaddressed). This pins the
    /// `&&` in the date guard: if it were `||`, an out-of-range month with an
    /// in-range day would wrongly emit a Daily coordinate.
    #[test]
    fn an_out_of_range_date_is_not_a_daily_episode() {
        // month 13 is impossible; day 14 is fine — `&&` must still reject.
        assert!(
            !coords("Show.2019.13.14.1080p.WEB-DL-GRP")
                .iter()
                .any(|c| matches!(c, Coordinates::Daily { .. })),
            "month 13 must not yield a Daily coordinate"
        );
        // day 32 is impossible; month 03 is fine — `&&` must still reject.
        assert!(
            !coords("Show.2019.03.32.1080p.WEB-DL-GRP")
                .iter()
                .any(|c| matches!(c, Coordinates::Daily { .. })),
            "day 32 must not yield a Daily coordinate"
        );
    }

    /// The word-form `Season N Episode A-B` range is honored only when it is a
    /// real ascending range (`a > 0 && b >= a && b - a < 500`). A descending
    /// range (`B < A`) is not a valid episode span and must NOT expand; it falls
    /// through to the season-pack reading. This pins the `b >= a` bound: were it
    /// `b < a`, a descending range would (wrongly) be accepted, and were `>=`
    /// loosened the boundary case would slip.
    #[test]
    fn descending_word_episode_range_is_rejected() {
        let got = coords("Show Season 1 Episode 5-4 x264");
        assert!(
            !got.iter().any(|c| matches!(c, Coordinates::Episode { .. })),
            "a descending episode range must not produce Episode coordinates, got {got:?}"
        );
    }

    /// The word-range guard also requires a positive first episode (`a > 0`): an
    /// `Episode 0-3` span is rejected (episode 0 is not a real episode number) and
    /// falls through to the season-pack reading. This pins the `a > 0` term of the
    /// guard — both the `>` operator and the `&&` joining it: an `a >= 0` or an
    /// `||` mutant would (wrongly) accept the range and emit episodes 0..3.
    #[test]
    fn word_episode_range_starting_at_zero_is_rejected() {
        let got = coords("Show Season 1 Episode 0-3 x264");
        assert!(
            !got.iter().any(|c| matches!(c, Coordinates::Episode { .. })),
            "an Episode 0-N range must not produce Episode coordinates, got {got:?}"
        );
    }

    /// The season-range guard requires a positive first season (`from > 0`): an
    /// `S00-S03` span is not expanded into four packs; it falls through to the
    /// single short-`S00` reading. This pins the `from > 0` term — a `from >= 0`
    /// mutant would expand a zero-based range.
    #[test]
    fn season_range_starting_at_zero_is_not_expanded() {
        // Only the single fall-through SeasonPack { season: 0 }, not 0,1,2,3.
        assert_eq!(
            coords("Show.S00-S03.1080p.WEB-DL-GRP"),
            vec![Coordinates::SeasonPack { season: 0 }]
        );
    }

    /// Both anime-absolute forms require a positive number (`n > 0`): an explicit
    /// ` - 0` (dash form) or a bracketed ` 0 ` (bare form) is not a real absolute
    /// episode and must yield NO Absolute coordinate. This pins the `n > 0` guard
    /// in both the bare (line 244) and dash (line 252) absolute branches.
    #[test]
    fn absolute_zero_is_rejected_in_both_forms() {
        assert!(
            !coords("[Grp] Show - 0 (1080p).mkv")
                .iter()
                .any(|c| matches!(c, Coordinates::Absolute { .. })),
            "absolute 0 (bracketed/bare form) must be rejected"
        );
        assert!(
            !coords("Some Show - 0 [1080p]")
                .iter()
                .any(|c| matches!(c, Coordinates::Absolute { .. })),
            "absolute 0 (dash form) must be rejected"
        );
    }

    /// The positive control for the word-range guard: an ascending in-range span
    /// expands to every episode inclusive. Together with the descending-range
    /// test this makes the `a > 0 && b >= a` guard load-bearing in both
    /// directions.
    #[test]
    fn ascending_word_episode_range_expands_inclusive() {
        assert_eq!(
            coords("Show Season 1 Episode 5-7 x264"),
            vec![
                Coordinates::Episode {
                    season: 1,
                    episode: 5,
                    absolute: None
                },
                Coordinates::Episode {
                    season: 1,
                    episode: 6,
                    absolute: None
                },
                Coordinates::Episode {
                    season: 1,
                    episode: 7,
                    absolute: None
                },
            ]
        );
    }

    /// A two-episode NxN multi block (`2x04-x05`) must yield BOTH episodes. The
    /// NxN-multi branch only commits when it produced at least two episodes
    /// (`episodes.len() >= 2`); this pins that `>= 2` bound — a `< 2` mutant would
    /// drop the multi block and fall through to the single-NxN reading (one
    /// episode), which this assertion rejects.
    #[test]
    fn nxn_multi_block_yields_both_episodes() {
        assert_eq!(
            coords("Show.2x04-x05.720p.HDTV"),
            vec![
                Coordinates::Episode {
                    season: 2,
                    episode: 4,
                    absolute: None
                },
                Coordinates::Episode {
                    season: 2,
                    episode: 5,
                    absolute: None
                },
            ]
        );
    }

    /// A season range `S01-S03` emits one season-pack per covered season, but only
    /// when the span is sane (`from > 0 && to >= from && to - from < 100`). A span
    /// of 199 seasons (`S01-S200`) is a runaway and must NOT expand into 200
    /// packs; it falls through to the single season-pack reading. This pins the
    /// `to - from < 100` runaway guard.
    #[test]
    fn season_range_expands_only_within_the_runaway_guard() {
        // In-range: S01-S03 -> exactly three packs.
        assert_eq!(
            coords("Show.S01-S03.1080p.WEB-DL-GRP"),
            vec![
                Coordinates::SeasonPack { season: 1 },
                Coordinates::SeasonPack { season: 2 },
                Coordinates::SeasonPack { season: 3 },
            ]
        );
        // Runaway: S01-S200 must NOT produce a 200-season expansion.
        let runaway = coords("Show.S01-S200.1080p.WEB-DL-GRP");
        assert!(
            runaway.len() < 100,
            "a 199-season span must not expand into a giant pack list, got {} packs",
            runaway.len()
        );
    }

    /// The dash-form anime absolute is bounded `n > 0 && n < 5000`: a four-digit
    /// number like 071 is accepted, but a number at/above 5000 is not a plausible
    /// absolute episode and must be ignored. This pins the `n < 5000` upper bound
    /// of the dash absolute. (No fansub bracket, so only the dash form can fire.)
    #[test]
    fn dash_absolute_is_bounded_below_5000() {
        assert_eq!(
            coords("Some Show - 071 [1080p]"),
            vec![Coordinates::Absolute { number: 71 }]
        );
        assert!(
            !coords("Some Show - 7000 [1080p]")
                .iter()
                .any(|c| matches!(c, Coordinates::Absolute { .. })),
            "an absolute >= 5000 must be rejected by the dash-form bound"
        );
    }

    /// `expand_episode_list` treats a two-number block as an inclusive range only
    /// when the span is bounded (`b - a < 200`); a larger gap is read as an
    /// explicit two-item list, never expanded. This pins the `< 200` runaway guard
    /// in the expander: `S01E01-E250` must be exactly [1, 250], not 250 episodes.
    #[test]
    fn two_number_block_with_a_huge_gap_is_a_list_not_a_range() {
        assert_eq!(
            coords("Show.S01E01-E250.720p.HDTV"),
            vec![
                Coordinates::Episode {
                    season: 1,
                    episode: 1,
                    absolute: None
                },
                Coordinates::Episode {
                    season: 1,
                    episode: 250,
                    absolute: None
                },
            ]
        );
    }

    /// `expand_episode_list` drops episode number 0 (`retain(|n| *n > 0)`): a
    /// multi block that includes `E00` (a specials marker some scene names use)
    /// must not surface episode 0 as a real episode. This pins the `*n > 0`
    /// retain predicate — a `>= 0` mutant would keep the bogus episode 0.
    #[test]
    fn episode_zero_is_dropped_from_a_multi_block() {
        assert_eq!(
            coords("Show.S01E00E01E02.720p.HDTV"),
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
            ]
        );
    }
}
