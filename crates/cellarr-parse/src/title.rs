//! Clean-title extraction.
//!
//! The title is the text before the first "structural" marker — the season/
//! episode tag, the year, the resolution, or the source. Everything from the
//! earliest such marker onward is the tag soup. Leading fansub brackets are
//! stripped first so anime titles come out clean.

use std::sync::LazyLock;

use cellarr_core::media::Coordinates;
use cellarr_core::parsed::ParsedRelease;
use regex::Regex;

// Leading `[Group]` fansub bracket(s) at the very start. Only SQUARE brackets are
// stripped: a leading `(...)` is far more often part of the title (`(500) Days of
// Summer`, `(Untitled)`) than a tag, and a leading `(year)` is rare — the year
// marker handles years wherever they appear.
static LEADING_TAGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\s*\[[^\]]*\]\s*)+").unwrap());

// A bare anime absolute number ending the title in fansub form: `Title 07`,
// `Title-01`, `Title.100`, `Title #957`. Only applied when a fansub bracket was
// stripped AND the numbering layer already found an Absolute coordinate, so this
// never cuts a number that is genuinely part of a title (e.g. `Apollo 13`).
static BARE_ABSOLUTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[\s._#-]+#?(\d{1,4})(?:v\d)?\b").unwrap());

// The earliest structural marker. Matched on the normalised string. Lookaround
// is avoided; this is a plain alternation.
static MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \b(
            s\d{1,3}[\s._-]*e\d{1,4}(?:[\s._-]*-?[\s._-]*e?\d{1,4})* | # S01E01[E02…]
            s\d{1,3}\b |                         # S01 season pack
            \d{3,4}x\d{3,4} |                    # 1280x720 dimensions (before NxN)
            \d{1,3}x\d{1,4} |                    # 1x05
            season[\s._-]*\d{1,3} |
            series[\s._-]*\d{1,3} |
            (?:saison|stagione|temporada|staffel|seizoen)[\s._-]*\d{1,3} |
            episode[\s._-]*\d{1,4} |             # word-form episode
            ep\d{1,4} |                          # Ep04 / EP06
            (?:19|20)\d{2} |                     # year / daily date start
            2160p? | 1080p? | 720p? | 576p? | 480p? | 4k | uhd |
            web[\s\-]?dl | webrip | bluray | blu[\s\-]ray | hdtv | bdrip | brrip |
            dvdrip | dvd | remux | x264 | x265 | hevc | h\.?26[45] | xvid | divx |
            # Release-modifier keywords: editions, language/dub tags, audio. Upstream
            # treats these as the start of the tag soup, not part of the title.
            extended | unrated | uncut | imax | remaster(?:ed)? | theatrical |
            director'?s | proper | repack | internal | limited | despecialized |
            # Compound/unambiguous language+dub tags only — a bare single-word
            # language (German/French/Italian) is too often part of a real title to
            # cut on, so those are left to the language extractor, not the title cut.
            truefrench | vostfr | subfrench | multi(?:sub)? | dubbed | subbed |
            ac3 | dts | aac | ddp?\d | flac | truehd | atmos
        )",
    )
    .unwrap()
});

// A four-digit year token, used to detect a "title number that looks like a
// year immediately followed by the real release year" (e.g. `Blade Runner 2049
// 2017 ...`), so the title keeps the embedded number.
// Separators tolerated between the title-year and the real year include brackets,
// so a bracketed real year (`1917 (2019)`, normalized to `1917 ( 2019 )`) is still
// recognized as the two-year case and the title keeps its number.
static TWO_YEARS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b((?:19|20)\d{2})[\s._\-()\[\]{}]*((?:19|20)\d{2})\b").unwrap()
});

// Release-modifier keywords (editions, language/dub, audio) can also legitimately
// BEGIN a real title — `Uncut Gems`, `Extended Family`, `Limited Partners`. A hard
// structural marker (year, season/episode tag, resolution, source, codec) never
// does. So when a marker sits at the very start of the remaining title text — which
// would otherwise strand the movie with an empty title — skip it if it's one of
// these soft words and cut at the next marker instead.
static SOFT_LEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)^\s*(
            extended | unrated | uncut | imax | remaster(?:ed)? | theatrical |
            director'?s | proper | repack | internal | limited | despecialized |
            truefrench | vostfr | subfrench | multi(?:sub)? | dubbed | subbed |
            ac3 | dts | aac | ddp?\d | flac | truehd | atmos
        )\b",
    )
    .unwrap()
});

// The effective title-cut point: the first structural marker that leaves real
// title text before it. A soft release-modifier word at the current title start
// (see `SOFT_LEADING`) is treated as part of the title and skipped.
fn find_cut(body: &str) -> Option<usize> {
    let mut from = 0;
    loop {
        let m = MARKER.find_at(body, from)?;
        let preceding = body[from..m.start()].trim();
        if preceding.is_empty() && SOFT_LEADING.is_match(&body[m.start()..]) {
            from = m.end();
            continue;
        }
        return Some(m.start());
    }
}

/// Extract a cleaned title.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    let norm = crate::tokens::normalize(input);

    // Strip leading bracketed groups/tags (anime fansub convention).
    let after_tags = LEADING_TAGS.replace(&norm, "");
    let had_leading_tag = after_tags.len() != norm.len();
    let body = after_tags.trim();

    // When the title ends in a number that itself looks like a year and the real
    // release year follows (`Blade Runner 2049 2017 …`), cut at the *second*
    // year so the embedded number stays in the title.
    let cut_at = find_cut(body);
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

    // Anime fansub form "[Group] Title - 071" / "[Group] Title 07": the title is
    // before the dash/absolute number. This ONLY applies in the anime context (a
    // fansub bracket was stripped AND the numbering layer recognized an Absolute) —
    // a movie's " - Subtitle" (`Ace Ventura - Pet Detective`) must survive, and a
    // number that is part of a real title is never severed.
    let has_absolute = out
        .coordinates
        .iter()
        .any(|c| matches!(c, Coordinates::Absolute { .. }));
    let title_part = if had_leading_tag && has_absolute {
        let before_dash = title_part.split(" - ").next().unwrap_or(title_part);
        match BARE_ABSOLUTE.find(before_dash) {
            Some(m) if m.start() > 0 => &before_dash[..m.start()],
            _ => before_dash,
        }
    } else {
        title_part
    };

    // Trim separators from both ends; additionally trim a TRAILING open-bracket left
    // when the year cut lands just after it (`10 Cloverfield Lane (` → year `(2016)`).
    // Leading brackets are kept so a `(500)`-style title prefix survives.
    let cleaned = title_part
        .trim()
        .trim_end_matches(|c: char| matches!(c, '-' | '.' | ' ' | '_' | '(' | '[' | '{'))
        .trim_start_matches(|c: char| matches!(c, '-' | '.' | ' ' | '_'))
        .trim();

    if !cleaned.is_empty() {
        out.clean_title = Some(cleaned.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Drive the FULL parse (not just `extract`) so the numbering layer runs first —
    // the anime "Title - 071" cut is gated on it having recognized an Absolute, so a
    // test that skips numbering would not reflect real usage.
    fn title(s: &str) -> Option<String> {
        crate::parse_title(s).clean_title
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

    // The library-onboarding dry-run over the real rinzler collection surfaced
    // these `Title (Year).ext` parse failures; the confident-match onboarding is
    // only as good as the title it looks up.

    #[test]
    fn keeps_dash_subtitle_for_a_movie() {
        // A " - " subtitle must survive (only anime fansub "Title - 071" cuts it).
        assert_eq!(
            title("Ace Ventura - Pet Detective (1994).mkv"),
            Some("Ace Ventura - Pet Detective".to_string())
        );
        assert_eq!(
            title("Alien - Covenant (2017).mkv"),
            Some("Alien - Covenant".to_string())
        );
    }

    #[test]
    fn edition_keyword_that_begins_a_real_title_survives() {
        // `Uncut` is a release-edition keyword, but here it's the actual title —
        // cutting at it stranded the movie with an empty title.
        assert_eq!(title("Uncut Gems (2019).mkv"), Some("Uncut Gems".to_string()));
        // A genuine edition tag AFTER a real title still cuts correctly.
        assert_eq!(
            title("Blade Runner 1982 The Final Cut 1080p BluRay"),
            Some("Blade Runner".to_string())
        );
        // The soft-keyword skip only rescues a leading edition word; a real edition
        // mid-name (`Extended`) after the title is still stripped.
        assert_eq!(
            title("Movie Title Extended Edition 1080p"),
            Some("Movie Title".to_string())
        );
    }

    #[test]
    fn numeric_title_survives_its_own_year_like_value() {
        // A title that IS a year-like number, followed by the real (bracketed) year.
        assert_eq!(title("1917 (2019).mkv"), Some("1917".to_string()));
        assert_eq!(title("2012 (2009).mkv"), Some("2012".to_string()));
        assert_eq!(title("300 (2006).mkv"), Some("300".to_string()));
    }

    #[test]
    fn no_trailing_open_bracket_from_the_year_cut() {
        assert_eq!(
            title("10 Cloverfield Lane (2016).mkv"),
            Some("10 Cloverfield Lane".to_string())
        );
    }
}
