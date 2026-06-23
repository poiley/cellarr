//! Custom-format scoring: the sum of the scores of all matching formats.

use cellarr_core::{CustomFormat, ParsedRelease, Release};

use crate::matching::MatchContext;

/// The custom formats that matched a release, paired with their scores, plus the
/// total. Returned by [`score_detailed`] so callers (and the decision log) can
/// explain *which* formats contributed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreBreakdown {
    /// The matching formats, in input order, with their (signed) scores.
    pub matched: Vec<MatchedFormat>,
    /// The arithmetic total: the sum of `matched` scores.
    pub total: i32,
}

/// One custom format that matched, with its score contribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedFormat {
    /// The format's human-facing name (stable enough for the decision log).
    pub name: String,
    /// The format's score, as contributed to the total.
    pub score: i32,
}

/// The total custom-format score for a release: the **sum** of the scores of
/// every matching custom format. Non-matching formats contribute nothing.
///
/// `i32` arithmetic is saturating so a pile of large guard scores can never wrap
/// into a positive total and accidentally pass a minimum-score gate.
#[must_use]
pub fn score(
    release: &Release,
    parsed: &ParsedRelease,
    formats: &[CustomFormat],
    ctx: &MatchContext<'_>,
) -> i32 {
    let mut total: i32 = 0;
    for format in formats {
        if ctx.matches(format, release, parsed) {
            total = total.saturating_add(format.score);
        }
    }
    total
}

/// Like [`score`] but also reports which formats matched, for explainability.
#[must_use]
pub fn score_detailed(
    release: &Release,
    parsed: &ParsedRelease,
    formats: &[CustomFormat],
    ctx: &MatchContext<'_>,
) -> ScoreBreakdown {
    let mut matched = Vec::new();
    let mut total: i32 = 0;
    for format in formats {
        if ctx.matches(format, release, parsed) {
            total = total.saturating_add(format.score);
            matched.push(MatchedFormat {
                name: format.name.clone(),
                score: format.score,
            });
        }
    }
    ScoreBreakdown { matched, total }
}
