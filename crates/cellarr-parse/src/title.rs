//! Clean-title extraction.
//!
//! The title is the text before the first "structural" marker — the season/
//! episode tag, the year, the resolution, or the source. Everything from the
//! earliest such marker onward is the tag soup. Leading fansub brackets are
//! stripped first so anime titles come out clean.

use std::sync::LazyLock;

use cellarr_core::parsed::ParsedRelease;
use regex::Regex;

// Leading bracketed tag(s): `[Group]`, `(2019)` etc. at the very start.
static LEADING_TAGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\s*[\[\(][^\]\)]*[\]\)]\s*)+").unwrap());

// The earliest structural marker. Matched on the normalised string. Lookaround
// is avoided; this is a plain alternation.
static MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \b(
            s\d{1,3}[\s._-]*e\d{1,4}(?:[\s._-]*-?[\s._-]*e?\d{1,4})* | # S01E01[E02…]
            s\d{1,3}\b |                         # S01 season pack
            \d{1,3}x\d{1,4} |                    # 1x05
            season[\s._-]*\d{1,3} |
            series[\s._-]*\d{1,3} |
            (?:19|20)\d{2} |                     # year / daily date start
            2160p? | 1080p? | 720p? | 576p? | 480p? | 4k | uhd |
            web[\s\-]?dl | webrip | bluray | blu[\s\-]ray | hdtv | bdrip | brrip |
            dvdrip | remux | x264 | x265 | hevc | h\.?26[45] | xvid
        )",
    )
    .unwrap()
});

// A four-digit year token, used to detect a "title number that looks like a
// year immediately followed by the real release year" (e.g. `Blade Runner 2049
// 2017 ...`), so the title keeps the embedded number.
static TWO_YEARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b((?:19|20)\d{2})[\s._-]+((?:19|20)\d{2})\b").unwrap());

/// Extract a cleaned title.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    let norm = crate::tokens::normalize(input);

    // Strip leading bracketed groups/tags (anime fansub convention).
    let after_tags = LEADING_TAGS.replace(&norm, "");
    let body = after_tags.trim();

    // When the title ends in a number that itself looks like a year and the real
    // release year follows (`Blade Runner 2049 2017 …`), cut at the *second*
    // year so the embedded number stays in the title.
    let cut_at = MARKER.find(body).map(|m| m.start());
    let cut_at = match (cut_at, TWO_YEARS.captures(body)) {
        (Some(marker_start), Some(years)) => {
            let first = years.get(1).unwrap();
            let second = years.get(2).unwrap();
            if marker_start == first.start() {
                Some(second.start())
            } else {
                Some(marker_start)
            }
        }
        (other, _) => other,
    };

    let title_part = match cut_at {
        Some(start) if start > 0 => &body[..start],
        Some(_) => "", // marker is at the very start, no title text
        None => body,
    };

    // For anime fansub form "[Group] Title - 071", the title is before the dash.
    let title_part = title_part.split(" - ").next().unwrap_or(title_part);

    let cleaned = title_part
        .trim()
        .trim_matches(|c: char| c == '-' || c == '.' || c == ' ' || c == '_')
        .trim();

    if !cleaned.is_empty() {
        out.clean_title = Some(cleaned.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(s: &str) -> Option<String> {
        let mut p = ParsedRelease::new(s);
        extract(s, &mut p);
        p.clean_title
    }

    #[test]
    fn tv_title() {
        assert_eq!(
            title("The.Show.S02E15.1080p.BluRay.x264-GROUP"),
            Some("The Show".to_string())
        );
    }

    #[test]
    fn movie_title_with_year() {
        assert_eq!(
            title("Movie.Title.2019.1080p.BluRay.x264-GRP"),
            Some("Movie Title".to_string())
        );
    }

    #[test]
    fn anime_fansub_title() {
        assert_eq!(
            title("[SubsPlease] Some Anime - 1071 (1080p) [ABCD1234].mkv"),
            Some("Some Anime".to_string())
        );
    }
}
