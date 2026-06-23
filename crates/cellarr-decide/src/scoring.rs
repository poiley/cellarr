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

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::{
        Condition, ConditionKind, CustomFormat, CustomFormatId, IndexerId, ParsedRelease, Protocol,
        Release, Source,
    };

    /// A custom format that matches when the parsed source equals `source`,
    /// contributing `score` to the total.
    fn source_cf(name: &str, source: Source, score: i32) -> CustomFormat {
        CustomFormat {
            id: CustomFormatId::new(),
            name: name.to_string(),
            conditions: vec![Condition {
                kind: ConditionKind::Source { source },
                required: false,
                negate: false,
            }],
            score,
        }
    }

    fn release(title: &str) -> Release {
        Release {
            indexer_id: IndexerId::new(),
            title: title.to_string(),
            download_url: String::new(),
            guid: None,
            protocol: Protocol::Torrent,
            size: None,
            seeders: None,
            indexer_flags: vec![],
        }
    }

    fn parsed_webdl() -> ParsedRelease {
        let mut p = ParsedRelease::new("x");
        p.source = Some(Source::WebDl);
        p
    }

    #[test]
    fn score_is_sum_of_matching_formats_only() {
        // Two match (WEB-DL), one does not (Bluray). The total is the sum of the
        // two that matched; the non-matcher contributes nothing.
        let fmts = vec![
            source_cf("web", Source::WebDl, 50),
            source_cf("web-bonus", Source::WebDl, 25),
            source_cf("bluray", Source::Bluray, 1000),
        ];
        let ctx = MatchContext::new(&fmts).expect("compiles");
        let total = score(&release("Show.WEB-DL"), &parsed_webdl(), &fmts, &ctx);
        assert_eq!(total, 75, "only the two WEB-DL formats contribute");
    }

    #[test]
    fn score_detailed_reports_matched_in_input_order_with_consistent_total() {
        let fmts = vec![
            source_cf("first", Source::WebDl, 10),
            source_cf("bluray", Source::Bluray, 999),
            source_cf("second", Source::WebDl, -3),
        ];
        let ctx = MatchContext::new(&fmts).expect("compiles");
        let breakdown = score_detailed(&release("Show.WEB-DL"), &parsed_webdl(), &fmts, &ctx);
        // The non-matching bluray format is absent; matched are in INPUT order.
        assert_eq!(breakdown.matched.len(), 2);
        assert_eq!(breakdown.matched[0].name, "first");
        assert_eq!(breakdown.matched[0].score, 10);
        assert_eq!(breakdown.matched[1].name, "second");
        assert_eq!(breakdown.matched[1].score, -3);
        // The breakdown total equals the sum of the reported matches AND equals
        // what the plain `score` function returns over the same inputs.
        assert_eq!(breakdown.total, 7);
        assert_eq!(
            breakdown.total,
            score(&release("Show.WEB-DL"), &parsed_webdl(), &fmts, &ctx),
            "score and score_detailed must agree on the total"
        );
        assert_eq!(
            breakdown.total,
            breakdown.matched.iter().map(|m| m.score).sum::<i32>(),
            "the reported total must equal the sum of the reported matches"
        );
    }

    #[test]
    fn empty_formats_score_zero_with_no_matches() {
        let fmts: Vec<CustomFormat> = vec![];
        let ctx = MatchContext::new(&fmts).expect("compiles");
        let breakdown = score_detailed(&release("anything"), &parsed_webdl(), &fmts, &ctx);
        assert_eq!(breakdown.total, 0);
        assert!(breakdown.matched.is_empty());
    }

    #[test]
    fn negative_scores_lower_the_total() {
        let fmts = vec![
            source_cf("good", Source::WebDl, 100),
            source_cf("penalty", Source::WebDl, -150),
        ];
        let ctx = MatchContext::new(&fmts).expect("compiles");
        let total = score(&release("x"), &parsed_webdl(), &fmts, &ctx);
        assert_eq!(total, -50, "a negative-score CF pulls the total below zero");
    }

    #[test]
    fn addition_saturates_instead_of_overflowing() {
        // A pile of large positive guard scores must clamp at i32::MAX rather than
        // wrap negative (which would let a huge positive total slip under a
        // minimum-score gate). Documented as the saturating-add contract.
        let fmts = vec![
            source_cf("a", Source::WebDl, i32::MAX),
            source_cf("b", Source::WebDl, i32::MAX),
            source_cf("c", Source::WebDl, i32::MAX),
        ];
        let ctx = MatchContext::new(&fmts).expect("compiles");
        assert_eq!(score(&release("x"), &parsed_webdl(), &fmts, &ctx), i32::MAX);

        // Symmetrically, a pile of large negatives clamps at i32::MIN, never
        // wrapping up into a passing positive.
        let neg = vec![
            source_cf("a", Source::WebDl, i32::MIN),
            source_cf("b", Source::WebDl, i32::MIN),
        ];
        let ctx2 = MatchContext::new(&neg).expect("compiles");
        assert_eq!(score(&release("x"), &parsed_webdl(), &neg, &ctx2), i32::MIN);
    }

    #[test]
    fn score_detailed_total_also_saturates() {
        let fmts = vec![
            source_cf("a", Source::WebDl, i32::MAX),
            source_cf("b", Source::WebDl, i32::MAX),
        ];
        let ctx = MatchContext::new(&fmts).expect("compiles");
        let breakdown = score_detailed(&release("x"), &parsed_webdl(), &fmts, &ctx);
        assert_eq!(breakdown.total, i32::MAX);
        // Both matched, even though their naive sum would have overflowed.
        assert_eq!(breakdown.matched.len(), 2);
    }
}
