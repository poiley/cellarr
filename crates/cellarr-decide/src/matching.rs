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
use regex::Regex;

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
                if let ConditionKind::ReleaseTitle { pattern } = &condition.kind {
                    if !regexes.contains_key(pattern) {
                        let re =
                            Regex::new(pattern).map_err(|source| DecideError::InvalidRegex {
                                format: format.name.clone(),
                                source,
                            })?;
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

    /// Whether `format` matches the release, applying the full TRaSH boolean
    /// algebra over every condition kind.
    #[must_use]
    pub fn matches(
        &self,
        format: &CustomFormat,
        release: &Release,
        parsed: &ParsedRelease,
    ) -> bool {
        let mut all_required = true;
        let mut any_optional = false;
        let mut have_optional = false;

        for condition in &format.conditions {
            let effective = self.condition_effective(condition, release, parsed);
            if condition.required {
                all_required &= effective;
            } else {
                have_optional = true;
                any_optional |= effective;
            }
        }

        all_required && (any_optional || !have_optional)
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
                let raw = self
                    .regexes
                    .get(pattern)
                    .is_some_and(|re| re.is_match(&release.title));
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
