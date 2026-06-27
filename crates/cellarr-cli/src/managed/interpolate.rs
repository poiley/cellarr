//! Secret interpolation: resolve `${ENV_VAR}` references from the process
//! environment **before** the YAML is deserialized.
//!
//! The managed-config file is committed to git (it lives next to the deployment,
//! e.g. a k8s ConfigMap), so secrets — indexer API keys, download-client
//! passwords — must never appear in it literally. Instead a string value can carry
//! `${NZBGEEK_KEY}` (or `${QBIT_PASS:-anonymous}` with a default), and this pass
//! substitutes the value from the process environment (where a k8s Secret injects
//! it). A referenced variable that is missing **and** has no default is a hard
//! error naming the variable, so a misconfigured deployment fails loudly rather
//! than silently authenticating with an empty key.
//!
//! Interpolation runs on the **raw file text** before parsing, so it is uniform
//! across every string value regardless of which section it sits in. A literal
//! dollar sign is escaped as `$$`.

use std::borrow::Cow;

use crate::managed::error::ManagedError;

/// Interpolate every `${VAR}` / `${VAR:-default}` reference in `input` against the
/// supplied `lookup` (typically [`std::env::var`]).
///
/// - `${VAR}` → the value of `VAR`; **error** if `VAR` is unset (no default).
/// - `${VAR:-default}` → the value of `VAR`, or `default` if `VAR` is unset/empty.
/// - `$$` → a literal `$` (the only escape; lets a value contain a real dollar).
/// - A bare `$` not followed by `{` or `$` is passed through untouched (so prices,
///   regexes, etc. survive).
///
/// `lookup` returns `Some(value)` for a set variable and `None` for an unset one.
/// This indirection keeps the function pure and unit-testable without touching the
/// real environment.
///
/// # Errors
/// Returns [`ManagedError::UnresolvedSecret`] naming the first variable that is
/// referenced without a default and is unset, or [`ManagedError::Interpolation`]
/// for a malformed reference (an unterminated `${`).
pub fn interpolate<F>(input: &str, mut lookup: F) -> Result<String, ManagedError>
where
    F: FnMut(&str) -> Option<String>,
{
    // Interpolation runs on the raw text before YAML parsing, so we must not treat
    // a `${...}` that appears inside a `#` comment as a secret reference — a
    // heavily commented config (including this repo's own example file) routinely
    // mentions the `${VAR}` syntax in prose. Process line by line: interpolate only
    // the code portion (before any comment) and pass the comment through verbatim.
    let mut out = String::with_capacity(input.len());
    for segment in input.split_inclusive('\n') {
        let (line, newline) = match segment.strip_suffix('\n') {
            Some(line) => (line, "\n"),
            None => (segment, ""),
        };
        match find_comment_start(line) {
            Some(idx) => {
                out.push_str(&interpolate_segment(&line[..idx], &mut lookup)?);
                out.push_str(&line[idx..]); // the comment passes through untouched
            }
            None => out.push_str(&interpolate_segment(line, &mut lookup)?),
        }
        out.push_str(newline);
    }
    Ok(out)
}

/// Interpolate every `${...}` / `$$` reference in a single non-comment text
/// segment (one line, up to any comment).
fn interpolate_segment<F>(input: &str, lookup: &mut F) -> Result<String, ManagedError>
where
    F: FnMut(&str) -> Option<String>,
{
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c != b'$' {
            // Push the raw byte; `input` is valid UTF-8 so byte-at-a-time append of
            // the matching char is safe via the original str slice.
            out.push(input[i..].chars().next().expect("non-empty slice"));
            i += input[i..].chars().next().expect("char").len_utf8();
            continue;
        }
        // We're at a '$'. Look at the next byte.
        match bytes.get(i + 1) {
            Some(b'$') => {
                // Escaped literal dollar.
                out.push('$');
                i += 2;
            }
            Some(b'{') => {
                // A `${...}` reference: find the matching `}`.
                let start = i + 2;
                let end = input[start..].find('}').ok_or_else(|| {
                    ManagedError::Interpolation(format!(
                        "unterminated `${{` reference starting at byte {i}"
                    ))
                })?;
                let expr = &input[start..start + end];
                let value = resolve_reference(expr, lookup)?;
                out.push_str(&value);
                i = start + end + 1; // skip past the '}'
            }
            _ => {
                // A bare '$' (price, regex anchor, …) — pass through untouched.
                out.push('$');
                i += 1;
            }
        }
    }
    Ok(out)
}

/// Find the byte index where a YAML comment begins on `line`, if any.
///
/// A `#` starts a comment when it is at the start of the line (after optional
/// whitespace) or is preceded by whitespace, **and** is not inside a quoted
/// scalar. This lets [`interpolate`] leave `${...}` written in comment prose
/// untouched while still substituting a `#` that sits inside a quoted value.
fn find_comment_start(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut prev_ws = true; // line start behaves like "preceded by whitespace"
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // A backslash escapes the next char inside a double-quoted scalar.
            b'\\' if in_double => {
                i += 1;
                prev_ws = false;
            }
            b'\'' if !in_double => {
                in_single = !in_single;
                prev_ws = false;
            }
            b'"' if !in_single => {
                in_double = !in_double;
                prev_ws = false;
            }
            b'#' if !in_single && !in_double && prev_ws => return Some(i),
            b' ' | b'\t' => prev_ws = true,
            _ => prev_ws = false,
        }
        i += 1;
    }
    None
}

/// Resolve one `${...}` body — either `VAR` or `VAR:-default`.
fn resolve_reference<F>(expr: &str, lookup: &mut F) -> Result<String, ManagedError>
where
    F: FnMut(&str) -> Option<String>,
{
    if let Some((name, default)) = expr.split_once(":-") {
        let name = name.trim();
        validate_var_name(name)?;
        // `:-` default semantics: unset OR empty falls back to the default.
        match lookup(name) {
            Some(v) if !v.is_empty() => Ok(v),
            _ => Ok(default.to_string()),
        }
    } else {
        let name = expr.trim();
        validate_var_name(name)?;
        lookup(name).ok_or_else(|| ManagedError::UnresolvedSecret {
            var: name.to_string(),
        })
    }
}

/// Reject an obviously malformed variable name (empty, or containing whitespace),
/// which usually means a typo'd `${ ... }` rather than a real reference.
fn validate_var_name(name: &str) -> Result<(), ManagedError> {
    if name.is_empty() {
        return Err(ManagedError::Interpolation(
            "empty `${}` variable reference".to_string(),
        ));
    }
    if name.chars().any(char::is_whitespace) {
        return Err(ManagedError::Interpolation(format!(
            "invalid environment variable name in reference: `{name}`"
        )));
    }
    Ok(())
}

/// A convenience [`Cow`] form: returns the input borrowed when it contains no
/// `$` at all (the common case), interpolated otherwise. Not currently used by
/// the loader (which always owns its text) but handy for callers that interpolate
/// many small strings.
#[allow(dead_code)]
pub fn interpolate_cow<'a, F>(input: &'a str, lookup: F) -> Result<Cow<'a, str>, ManagedError>
where
    F: FnMut(&str) -> Option<String>,
{
    if !input.contains('$') {
        return Ok(Cow::Borrowed(input));
    }
    interpolate(input, lookup).map(Cow::Owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl FnMut(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn substitutes_present_variable() {
        let out = interpolate("apiKey: ${KEY}", env(&[("KEY", "secret")])).unwrap();
        assert_eq!(out, "apiKey: secret");
    }

    #[test]
    fn missing_required_variable_is_an_error_naming_it() {
        let err = interpolate("apiKey: ${KEY}", env(&[])).unwrap_err();
        match err {
            ManagedError::UnresolvedSecret { var } => assert_eq!(var, "KEY"),
            other => panic!("expected UnresolvedSecret, got {other:?}"),
        }
    }

    #[test]
    fn default_used_when_unset() {
        let out = interpolate("user: ${USER:-anonymous}", env(&[])).unwrap();
        assert_eq!(out, "user: anonymous");
    }

    #[test]
    fn default_used_when_empty() {
        let out = interpolate("user: ${USER:-anonymous}", env(&[("USER", "")])).unwrap();
        assert_eq!(out, "user: anonymous");
    }

    #[test]
    fn present_overrides_default() {
        let out = interpolate("user: ${USER:-anonymous}", env(&[("USER", "real")])).unwrap();
        assert_eq!(out, "user: real");
    }

    #[test]
    fn escaped_dollar_is_literal() {
        // `$$` -> `$`; a literal price survives without being treated as a ref.
        let out = interpolate("note: costs $$5 ${OK}", env(&[("OK", "ok")])).unwrap();
        assert_eq!(out, "note: costs $5 ok");
    }

    #[test]
    fn bare_dollar_passes_through() {
        // A `$` not followed by `{` or `$` is left alone (regex anchors, etc.).
        let out = interpolate("pattern: ^foo$bar", env(&[])).unwrap();
        assert_eq!(out, "pattern: ^foo$bar");
    }

    #[test]
    fn multiple_references_in_one_line() {
        let out = interpolate("${A}-${B:-z}-${A}", env(&[("A", "x")])).unwrap();
        assert_eq!(out, "x-z-x");
    }

    #[test]
    fn unterminated_reference_is_an_error() {
        let err = interpolate("apiKey: ${KEY", env(&[("KEY", "v")])).unwrap_err();
        assert!(matches!(err, ManagedError::Interpolation(_)));
    }

    #[test]
    fn empty_reference_is_an_error() {
        let err = interpolate("x: ${}", env(&[])).unwrap_err();
        assert!(matches!(err, ManagedError::Interpolation(_)));
    }

    #[test]
    fn full_line_comment_placeholder_is_not_interpolated() {
        // `${VAR}` mentioned in comment prose must NOT be treated as a secret ref
        // (so a commented example file validates without those vars being set).
        let out = interpolate("# secrets use ${ENV} syntax, e.g. ${VAR}", env(&[])).unwrap();
        assert_eq!(out, "# secrets use ${ENV} syntax, e.g. ${VAR}");
    }

    #[test]
    fn trailing_comment_placeholder_is_ignored_but_value_interpolates() {
        let out = interpolate(
            "apiKey: ${KEY} # set this to your ${NZBGEEK_KEY}",
            env(&[("KEY", "secret")]),
        )
        .unwrap();
        assert_eq!(out, "apiKey: secret # set this to your ${NZBGEEK_KEY}");
    }

    #[test]
    fn hash_inside_quotes_is_not_a_comment() {
        // A `#` inside a quoted scalar is part of the value, so its `${...}` still
        // interpolates and the `#` is not mistaken for a comment.
        let out = interpolate(r#"name: "a # ${B}""#, env(&[("B", "x")])).unwrap();
        assert_eq!(out, r#"name: "a # x""#);
    }

    #[test]
    fn comment_lines_interleaved_with_values() {
        let input = "a: ${A}\n# note: ${UNSET_OK}\nb: ${B}";
        let out = interpolate(input, env(&[("A", "1"), ("B", "2")])).unwrap();
        assert_eq!(out, "a: 1\n# note: ${UNSET_OK}\nb: 2");
    }
}
