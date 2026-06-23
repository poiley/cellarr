//! Shared text helpers used by several extractors.
//!
//! Release names use `.`, `_`, and space interchangeably as separators. Most
//! matching is far easier on a normalized form where every separator is a
//! single space, so callers can write word-boundary patterns without caring
//! which separator a particular scene group preferred.

use std::sync::LazyLock;

use regex::Regex;

/// Collapse the common release-name separators (`.`, `_`, multiple spaces) into
/// single spaces. Brackets are surrounded by spaces so bracketed tags become
/// their own whitespace-delimited tokens.
///
/// The result is lossy with respect to the exact separators but preserves token
/// order and case, which is all the extractors need.
#[must_use]
pub(crate) fn normalize(input: &str) -> String {
    static MULTISPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

    let mut s = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '.' | '_' => s.push(' '),
            '[' | ']' | '(' | ')' | '{' | '}' => {
                s.push(' ');
                s.push(ch);
                s.push(' ');
            }
            _ => s.push(ch),
        }
    }
    MULTISPACE.replace_all(s.trim(), " ").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_separators() {
        assert_eq!(normalize("The.Show_Name  Here"), "The Show Name Here");
    }

    #[test]
    fn isolates_brackets() {
        assert_eq!(normalize("[Grp]Show-01"), "[ Grp ] Show-01");
    }
}
