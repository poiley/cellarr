//! Full custom-format matching over a [`Release`] plus its parse.
//!
//! `cellarr-core` owns the per-condition boolean algebra (OR by default,
//! `required` = AND, `negate` = absence) but deliberately cannot evaluate two
//! condition kinds: [`ConditionKind::ReleaseTitle`] (no regex dependency in
//! core) and [`ConditionKind::IndexerFlag`] / [`ConditionKind::Size`] (those
//! facts live on the [`Release`], not the [`ParsedRelease`]). This module
//! supplies the missing evaluation by compiling title regexes against the raw
//! title and reading indexer flags / size from the release, then delegating the
//! `required`/`negate`/OR combination to the same rules core encodes.

use std::collections::HashMap;

use cellarr_core::{
    condition_matches, Condition, ConditionKind, CustomFormat, ParsedRelease, Release,
};
use fancy_regex::Regex;

use crate::error::DecideError;

/// A compiled view of the custom formats: every title regex compiled once.
///
/// Compiling is the only fallible part of matching, so it is hoisted into a
/// constructor; afterwards [`MatchContext::matches`] and scoring are infallible.
#[derive(Debug)]
pub struct MatchContext<'a> {
    formats: &'a [CustomFormat],
    /// Compiled title regexes, keyed by the pattern string. Patterns repeat
    /// across formats (TRaSH reuses regexes), so deduplicating by pattern keeps
    /// compilation linear in distinct patterns.
    regexes: HashMap<String, Regex>,
}

/// Compile one custom-format title pattern the way the decision engine does.
///
/// Custom-format title regexes are matched CASE-INSENSITIVELY, matching
/// Sonarr/Radarr (which compile CF regexes with IgnoreCase). TRaSH custom
/// formats are written lowercase and rely on this; without it cellarr would
/// match almost no real-world CFs (e.g. "HEVC"/"REPACK"/"AMZN") and make wrong
/// grab decisions. The `(?i)` group flag applies IgnoreCase to the whole
/// pattern.
///
/// Compilation uses fancy-regex, not the linear-time `regex` crate, because real
/// TRaSH CFs use .NET look-around (look-ahead/look-behind) — which `regex`
/// rejects but fancy-regex (like the apps' .NET engine) accepts. A small tail of
/// CFs still use .NET-only constructs fancy-regex cannot model (variable-size
/// look-behind, `[` inside a character class); those fail here and are reported,
/// never silently mismatched.
///
/// # Errors
/// Returns [`DecideError::InvalidRegex`] (naming `format_name`) if `pattern`
/// does not compile under cellarr's engine.
pub fn compile_title_regex(format_name: &str, pattern: &str) -> Result<Regex, DecideError> {
    Regex::new(&format!("(?i){pattern}")).map_err(|source| DecideError::InvalidRegex {
        format: format_name.to_string(),
        source: Box::new(source),
    })
}

/// Whether every regex-bearing condition in `format` compiles under cellarr's
/// engine — i.e. whether `format` can take part in a [`MatchContext`].
///
/// Both [`ConditionKind::ReleaseTitle`] and [`ConditionKind::ReleaseGroup`] carry
/// a regex (the apps compile the release-group spec's value as a regex against the
/// parsed group, not as an exact-equality), so both must compile.
///
/// Used by the tolerant TRaSH import to skip-and-count CFs carrying a
/// dialect-incompatible regex rather than fail the whole import.
#[must_use]
pub fn format_regexes_compile(format: &CustomFormat) -> bool {
    format.conditions.iter().all(|c| match &c.kind {
        ConditionKind::ReleaseTitle { pattern } | ConditionKind::ReleaseGroup { name: pattern } => {
            compile_title_regex(&format.name, pattern).is_ok()
        }
        _ => true,
    })
}

impl<'a> MatchContext<'a> {
    /// Compile every release-title regex in `formats`.
    ///
    /// # Errors
    /// Returns [`DecideError::InvalidRegex`] if any title pattern fails to
    /// compile, naming the offending custom format.
    pub fn new(formats: &'a [CustomFormat]) -> Result<Self, DecideError> {
        let mut regexes = HashMap::new();
        for format in formats {
            for condition in &format.conditions {
                // Both ReleaseTitle and ReleaseGroup carry a regex (the apps
                // compile the group spec's value as a case-insensitive regex
                // against the parsed release group, not an exact-equality).
                if let ConditionKind::ReleaseTitle { pattern }
                | ConditionKind::ReleaseGroup { name: pattern } = &condition.kind
                {
                    if !regexes.contains_key(pattern) {
                        let re = compile_title_regex(&format.name, pattern)?;
                        regexes.insert(pattern.clone(), re);
                    }
                }
            }
        }
        Ok(Self { formats, regexes })
    }

    /// The custom formats this context was built over.
    #[must_use]
    pub fn formats(&self) -> &'a [CustomFormat] {
        self.formats
    }

    /// Whether `format` matches the release, applying the Servarr custom-format
    /// boolean algebra (verified live against Sonarr/Radarr `/api/v3/parse`):
    ///
    /// * **Required** conditions are pure AND — *each* must match individually,
    ///   regardless of its implementation (two required conditions of the same
    ///   kind do **not** OR; both must hold).
    /// * **Non-required** conditions are grouped by *implementation* (the
    ///   condition kind): within a group they OR, and across groups they AND.
    ///   Every implementation that has at least one non-required condition must
    ///   contribute at least one match.
    ///
    /// The format matches when every required condition holds **and** every
    /// non-required implementation-group is satisfied. With no non-required
    /// conditions, the group test is vacuously true.
    ///
    /// This implementation-grouped semantics is the key divergence from a naive
    /// flat-OR: e.g. an anime "tier" CF that lists `Source=web` plus a set of
    /// release-group regexes (all non-required) only matches a WEB release whose
    /// group is *also* in the list — not every WEB release.
    #[must_use]
    pub fn matches(
        &self,
        format: &CustomFormat,
        release: &Release,
        parsed: &ParsedRelease,
    ) -> bool {
        // Per non-required implementation group: (saw any member, any matched).
        let mut groups: HashMap<std::mem::Discriminant<ConditionKind>, bool> = HashMap::new();

        for condition in &format.conditions {
            let effective = self.condition_effective(condition, release, parsed);
            if condition.required {
                // Pure AND across required conditions.
                if !effective {
                    return false;
                }
            } else {
                let key = std::mem::discriminant(&condition.kind);
                let entry = groups.entry(key).or_insert(false);
                *entry |= effective;
            }
        }

        // Every non-required implementation group must have at least one match.
        groups.values().all(|matched| *matched)
    }

    /// One condition's effective result (raw fact XOR `negate`), evaluating the
    /// kinds core cannot and delegating the rest to core so the semantics stay
    /// single-sourced.
    fn condition_effective(
        &self,
        condition: &Condition,
        release: &Release,
        parsed: &ParsedRelease,
    ) -> bool {
        match &condition.kind {
            ConditionKind::ReleaseTitle { pattern } => {
                // fancy-regex's `is_match` is fallible (a backtracking blow-up
                // surfaces as Err); a non-match and an evaluation error are both
                // "does not match" for decision purposes.
                let raw = self
                    .regexes
                    .get(pattern)
                    .is_some_and(|re| re.is_match(&release.title).unwrap_or(false));
                raw ^ condition.negate
            }
            ConditionKind::ReleaseGroup { name: pattern } => {
                // The apps treat the release-group spec value as a regex matched
                // against the parsed group (case-insensitive), not exact equality.
                // TRaSH relies on this — e.g. `No-RlsGroup` is `ReleaseGroup` =
                // `.` negated (matches when there is NO group at all). An absent
                // group can never match a regex, so the raw result is false.
                let raw = parsed.group.as_deref().is_some_and(|g| {
                    self.regexes
                        .get(pattern)
                        .is_some_and(|re| re.is_match(g).unwrap_or(false))
                });
                raw ^ condition.negate
            }
            ConditionKind::IndexerFlag { flag } => {
                let raw = release
                    .indexer_flags
                    .iter()
                    .any(|f| f.eq_ignore_ascii_case(flag));
                raw ^ condition.negate
            }
            ConditionKind::Size { min, max } => {
                let raw = release.size.is_some_and(|size| {
                    min.is_none_or(|lo| size >= lo) && max.is_none_or(|hi| size <= hi)
                });
                raw ^ condition.negate
            }
            // Every other kind reads only from the parse; core owns that algebra
            // (including negate), so defer to it to avoid a second copy of the
            // semantics drifting out of sync.
            _ => condition_matches(condition, parsed, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::{Confidence, CustomFormatId, IndexerId, ParsedField, Protocol};

    fn title_cf(name: &str, pattern: &str) -> CustomFormat {
        CustomFormat {
            id: CustomFormatId::new(),
            name: name.to_string(),
            conditions: vec![Condition {
                kind: ConditionKind::ReleaseTitle {
                    pattern: pattern.to_string(),
                },
                required: true,
                negate: false,
            }],
            score: 0,
        }
    }

    fn rel(title: &str) -> (Release, ParsedRelease) {
        let mut p = ParsedRelease::new(title);
        p.set_confidence(ParsedField::Resolution, Confidence::new(0.9));
        (
            Release {
                indexer_id: IndexerId::new(),
                title: title.to_string(),
                download_url: String::new(),
                guid: None,
                protocol: Protocol::Torrent,
                size: None,
                seeders: None,
                indexer_flags: vec![],
            },
            p,
        )
    }

    #[test]
    fn release_title_regex_is_case_insensitive() {
        // TRaSH CFs are written lowercase and rely on case-insensitive matching,
        // as Sonarr/Radarr do. Verified against the live apps by the CF oracle.
        let fmts = vec![
            title_cf("hevc", r"(x265|h265|hevc)"),
            title_cf("proper", r"\bproper\b"),
        ];
        let ctx = MatchContext::new(&fmts).expect("compiles");
        let (r, p) = rel("Show.S01E01.PROPER.1080p.WEB-DL.HEVC-GRP");
        assert!(
            ctx.matches(&fmts[0], &r, &p),
            "uppercase HEVC must match lowercase pattern"
        );
        assert!(ctx.matches(&fmts[1], &r, &p), "uppercase PROPER must match");
    }

    fn cond(kind: ConditionKind, required: bool, negate: bool) -> Condition {
        Condition {
            kind,
            required,
            negate,
        }
    }

    fn cf(name: &str, conditions: Vec<Condition>) -> CustomFormat {
        CustomFormat {
            id: CustomFormatId::new(),
            name: name.to_string(),
            conditions,
            score: 0,
        }
    }

    #[test]
    fn non_required_conditions_or_within_implementation_and_across() {
        // Mirrors a TRaSH "tier" CF: a Source group OR'd with a ReleaseTitle
        // group, all non-required. Verified live: such a CF matches only when
        // BOTH the source AND the title conditions are satisfied (cross-impl AND),
        // not when just one source matches (the old flat-OR bug over-matched).
        use cellarr_core::Source;
        let format = cf(
            "tier",
            vec![
                cond(
                    ConditionKind::Source {
                        source: Source::WebDl,
                    },
                    false,
                    false,
                ),
                cond(
                    ConditionKind::Source {
                        source: Source::Webrip,
                    },
                    false,
                    false,
                ),
                cond(
                    ConditionKind::ReleaseTitle {
                        pattern: r"\b(GoodGroup)\b".into(),
                    },
                    false,
                    false,
                ),
            ],
        );
        let ctx = MatchContext::new(std::slice::from_ref(&format)).expect("compiles");

        // WEB-DL but group not in the title-list -> source group ok, title group
        // empty -> AND fails -> no match.
        let (r, mut p) = rel("Show.S01E01.1080p.WEB-DL.x264-OtherGroup");
        p.source = Some(Source::WebDl);
        assert!(
            !ctx.matches(&format, &r, &p),
            "source-only must NOT match a tier CF (cross-impl AND)"
        );

        // WEB-DL AND the title group present -> both groups satisfied -> match.
        let (r2, mut p2) = rel("Show.S01E01.1080p.WEB-DL.x264-GoodGroup");
        p2.source = Some(Source::WebDl);
        assert!(
            ctx.matches(&format, &r2, &p2),
            "source AND title both satisfied must match"
        );
    }

    #[test]
    fn required_conditions_are_pure_and_even_same_implementation() {
        // Two REQUIRED ReleaseTitle conditions: both must match individually
        // (they do NOT OR just because they share an implementation). Verified
        // live against Sonarr.
        let format = cf(
            "two-required-titles",
            vec![
                cond(
                    ConditionKind::ReleaseTitle {
                        pattern: r"web-dl".into(),
                    },
                    true,
                    false,
                ),
                cond(
                    ConditionKind::ReleaseTitle {
                        pattern: r"zzznevermatch".into(),
                    },
                    true,
                    false,
                ),
            ],
        );
        let ctx = MatchContext::new(std::slice::from_ref(&format)).expect("compiles");
        let (r, p) = rel("Show.S01E01.1080p.WEB-DL.x264-GRP");
        assert!(
            !ctx.matches(&format, &r, &p),
            "one required title missing must fail (required = pure AND)"
        );
    }

    #[test]
    fn release_group_is_regex_matched_not_exact() {
        // The apps compile the ReleaseGroup spec value as a regex against the
        // parsed group. `No-RlsGroup` = ReleaseGroup `.` negated -> matches only
        // when there is NO group at all (an absent group can't match any regex).
        let no_group = cf(
            "No-RlsGroup",
            vec![cond(
                ConditionKind::ReleaseGroup { name: ".".into() },
                false,
                true,
            )],
        );
        let ctx = MatchContext::new(std::slice::from_ref(&no_group)).expect("compiles");

        // Has a group -> regex `.` matches the group -> negate -> no match.
        let (r, mut p) = rel("Show.S01E01.1080p.WEB-DL-GRP");
        p.group = Some("GRP".into());
        assert!(
            !ctx.matches(&no_group, &r, &p),
            "a release WITH a group must not match No-RlsGroup"
        );

        // No group -> regex can't match absent group -> negate -> match.
        let (r2, p2) = rel("Show.S01E01.1080p.WEB-DL");
        assert!(
            ctx.matches(&no_group, &r2, &p2),
            "a release with NO group must match No-RlsGroup"
        );

        // A prefix/substring group regex matches anywhere in the group (regex,
        // not exact-equality): `ECLiPSE` matches a group "ECLiPSE".
        let eclipse = cf(
            "asian-ish",
            vec![cond(
                ConditionKind::ReleaseGroup {
                    name: "^(ECLiPSE)$".into(),
                },
                false,
                false,
            )],
        );
        let ctx2 = MatchContext::new(std::slice::from_ref(&eclipse)).expect("compiles");
        let (r3, mut p3) = rel("Movie.2020.1080p.WEB-DL-ECLiPSE");
        p3.group = Some("ECLiPSE".into());
        assert!(ctx2.matches(&eclipse, &r3, &p3), "regex group must match");
        p3.group = Some("NotEclipse".into());
        assert!(
            !ctx2.matches(&eclipse, &r3, &p3),
            "anchored regex must not match a different group"
        );
    }
}
