//! Release-profile term evaluation over a release title.
//!
//! `cellarr-core` owns the [`ReleaseProfile`] data model and the plain-substring
//! / `/regex/` term-shape semantics ([`plain_term_matches`], [`regex_term`]) but
//! deliberately cannot evaluate the regex form (no regex dependency in core).
//! This module supplies that missing half — compiling `/pattern/` terms with the
//! same case-insensitive fancy-regex engine custom formats use — and combines a
//! profile's required / ignored / preferred terms into the verdict the decision
//! engine consumes.
//!
//! Semantics (mirroring Sonarr's release profile):
//! * A release containing **any** ignored term is rejected.
//! * If a profile lists **required** terms, the release must contain at least one
//!   of them, else it is rejected.
//! * Every matching **preferred** term adds its score to the release's total.
//!
//! Only **enabled** profiles whose tags apply to the content are evaluated; that
//! filtering is the caller's (the decision engine resolves which profiles apply
//! via [`ReleaseProfile::applies_to`]).

use cellarr_core::{plain_term_matches, regex_term, ReleaseProfile};

use crate::matching::compile_title_regex;

/// What a release profile says about a release title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseProfileVerdict {
    /// Rejected because the title matched an ignored ("must not contain") term.
    /// Carries the matched term for the decision log.
    Ignored {
        /// The ignored term that matched.
        term: String,
    },
    /// Rejected because the profile has required ("must contain") terms but the
    /// title matched none of them.
    RequiredMissing,
    /// Accepted by this profile, contributing `preferred_score` to the release's
    /// total (the sum of every matching preferred term's score; `0` when none
    /// match or none are configured).
    Accept {
        /// The summed preferred-term score this profile contributes.
        preferred_score: i32,
    },
}

/// Whether a single term (plain substring or `/regex/`) matches `title`,
/// case-insensitively.
///
/// Plain terms defer to core's [`plain_term_matches`]; `/pattern/` terms compile
/// under the same case-insensitive fancy-regex engine custom-format title
/// patterns use. A pattern that fails to compile, or whose evaluation errors
/// (a backtracking blow-up surfaces as `Err`), is treated as **not matching** —
/// the same conservative stance `cellarr-decide` takes for CF title regexes, so a
/// bad pattern never spuriously rejects or scores a release.
#[must_use]
pub fn term_matches(term: &str, title: &str) -> bool {
    match regex_term(term) {
        Some(pattern) => compile_title_regex("release-profile", pattern)
            .ok()
            .is_some_and(|re| re.is_match(title).unwrap_or(false)),
        None => plain_term_matches(term, title),
    }
}

/// Evaluate one release profile against a release `title`.
///
/// Precedence within a profile: ignored terms reject first (a "must not contain"
/// hit is final), then a required-but-unmatched profile rejects, otherwise the
/// profile accepts and contributes its summed preferred-term score.
///
/// The caller is responsible for only passing **enabled** profiles whose tags
/// apply to the content; this function does not re-check those (it evaluates the
/// term lists verbatim).
#[must_use]
pub fn evaluate_release_profile(profile: &ReleaseProfile, title: &str) -> ReleaseProfileVerdict {
    // Ignored terms reject first — "must not contain" is final.
    for term in &profile.ignored {
        if term_matches(term, title) {
            return ReleaseProfileVerdict::Ignored { term: term.clone() };
        }
    }

    // Required terms: when any are configured, at least one must match.
    if !profile.required.is_empty() && !profile.required.iter().any(|t| term_matches(t, title)) {
        return ReleaseProfileVerdict::RequiredMissing;
    }

    // Preferred terms: sum the score of every matching term (saturating so a pile
    // of large scores can never wrap).
    let mut preferred_score: i32 = 0;
    for pref in &profile.preferred {
        if term_matches(&pref.term, title) {
            preferred_score = preferred_score.saturating_add(pref.score);
        }
    }
    ReleaseProfileVerdict::Accept { preferred_score }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::PreferredTerm;

    fn profile() -> ReleaseProfile {
        ReleaseProfile::new("test")
    }

    #[test]
    fn ignored_substring_rejects() {
        let mut p = profile();
        p.ignored = vec!["x265".into()];
        assert_eq!(
            evaluate_release_profile(&p, "Show.S01E01.1080p.WEB-DL.x265-GRP"),
            ReleaseProfileVerdict::Ignored {
                term: "x265".into()
            }
        );
        // No ignored hit -> accept.
        assert_eq!(
            evaluate_release_profile(&p, "Show.S01E01.1080p.WEB-DL.x264-GRP"),
            ReleaseProfileVerdict::Accept { preferred_score: 0 }
        );
    }

    #[test]
    fn ignored_regex_rejects() {
        let mut p = profile();
        p.ignored = vec!["/x26[45]/".into()];
        assert_eq!(
            evaluate_release_profile(&p, "Show.x264-GRP"),
            ReleaseProfileVerdict::Ignored {
                term: "/x26[45]/".into()
            }
        );
    }

    #[test]
    fn required_present_passes_absent_rejects() {
        let mut p = profile();
        p.required = vec!["bluray".into()];
        // Present (case-insensitive) -> accept.
        assert_eq!(
            evaluate_release_profile(&p, "Movie.2020.1080p.BluRay.x264-GRP"),
            ReleaseProfileVerdict::Accept { preferred_score: 0 }
        );
        // Absent -> reject.
        assert_eq!(
            evaluate_release_profile(&p, "Movie.2020.1080p.WEB-DL.x264-GRP"),
            ReleaseProfileVerdict::RequiredMissing
        );
    }

    #[test]
    fn preferred_sums_matching_scores() {
        let mut p = profile();
        p.preferred = vec![
            PreferredTerm {
                term: "remux".into(),
                score: 50,
            },
            PreferredTerm {
                term: "/atmos/".into(),
                score: 25,
            },
            PreferredTerm {
                term: "cam".into(),
                score: -100,
            },
        ];
        // remux + atmos match (75), cam does not.
        assert_eq!(
            evaluate_release_profile(&p, "Movie.2020.2160p.Remux.Atmos-GRP"),
            ReleaseProfileVerdict::Accept {
                preferred_score: 75
            }
        );
        // A negative preferred term demotes.
        assert_eq!(
            evaluate_release_profile(&p, "Movie.2020.CAM-GRP"),
            ReleaseProfileVerdict::Accept {
                preferred_score: -100
            }
        );
    }

    #[test]
    fn ignored_beats_required_and_preferred() {
        let mut p = profile();
        p.required = vec!["bluray".into()];
        p.ignored = vec!["x265".into()];
        p.preferred = vec![PreferredTerm {
            term: "bluray".into(),
            score: 10,
        }];
        // Even though required and preferred match, the ignored term rejects.
        assert_eq!(
            evaluate_release_profile(&p, "Movie.2020.BluRay.x265-GRP"),
            ReleaseProfileVerdict::Ignored {
                term: "x265".into()
            }
        );
    }

    #[test]
    fn bad_regex_never_matches() {
        let mut p = profile();
        // An unbalanced group never compiles -> never matches -> no reject.
        p.ignored = vec!["/(unclosed/".into()];
        assert_eq!(
            evaluate_release_profile(&p, "anything-(unclosed"),
            ReleaseProfileVerdict::Accept { preferred_score: 0 }
        );
    }
}
