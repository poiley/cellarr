//! Year extraction.
//!
//! A four-digit `19xx`/`20xx` token is a strong year signal, but the same digits
//! can appear in resolutions (`2160`, `1080`) or be part of a title (`2012`,
//! `1984`). We restrict to the plausible release-year window and, when several
//! candidates exist, prefer the one that is *parenthesised* (movie convention)
//! or the last one before the quality tags.

use std::sync::LazyLock;

use cellarr_core::parsed::{Confidence, ParsedField, ParsedRelease};
use regex::Regex;

// A four-digit token in a sane range, optionally wrapped in parens/brackets.
static YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?x) [\(\[]? \b(19\d{2}|20\d{2})\b [\)\]]?").unwrap());

/// The latest plausible release year. Anything beyond this is almost certainly
/// not a year (it is a resolution or noise).
const MAX_YEAR: u16 = 2099;
const MIN_YEAR: u16 = 1900;

/// Extract the year.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    let norm = crate::tokens::normalize(input);

    // Collect all candidates with whether they were parenthesised.
    let mut candidates: Vec<(u16, bool, usize)> = Vec::new();
    for m in YEAR.captures_iter(&norm) {
        let full = m.get(0).map(|x| x.as_str()).unwrap_or("");
        let Some(yr_match) = m.get(1) else { continue };
        let Ok(yr) = yr_match.as_str().parse::<u16>() else {
            continue;
        };
        if !(MIN_YEAR..=MAX_YEAR).contains(&yr) {
            continue;
        }
        let parenthesised = full.starts_with('(') || full.starts_with('[');
        candidates.push((yr, parenthesised, yr_match.start()));
    }
    if candidates.is_empty() {
        return;
    }

    // Prefer a parenthesised year; otherwise the last candidate (titles that
    // *are* a year, like "2012", put the real release year later).
    let chosen = candidates
        .iter()
        .find(|(_, paren, _)| *paren)
        .copied()
        .unwrap_or_else(|| *candidates.last().unwrap());

    out.year = Some(chosen.0);
    let conf = if chosen.1 { 0.98 } else { 0.85 };
    out.set_confidence(ParsedField::Year, Confidence::new(conf));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn year(s: &str) -> Option<u16> {
        let mut p = ParsedRelease::new(s);
        extract(s, &mut p);
        p.year
    }

    #[test]
    fn basic_movie_year() {
        assert_eq!(year("Movie.Title.2019.1080p.BluRay.x264"), Some(2019));
    }

    #[test]
    fn parenthesised_wins() {
        assert_eq!(year("Movie (2008) 1080p BluRay"), Some(2008));
    }

    #[test]
    fn title_is_a_year_then_real_year() {
        // "2012" the title, "2009" the release year.
        assert_eq!(year("2012.2009.1080p.BluRay.x264"), Some(2009));
    }

    #[test]
    fn none_when_absent() {
        assert_eq!(year("Show.S01E01.1080p.WEB-DL.x264"), None);
    }
}
