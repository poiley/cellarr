//! The rename engine: deterministic on-disk names from naming tokens.
//!
//! The pipeline never decides on-disk names itself; it asks the `MediaModule`
//! for [`NamingTokens`](cellarr_core::NamingTokens) and feeds them, together with
//! the user's naming *format*, to [`render_name`]. The transform is pure and
//! deterministic: the same tokens and format always yield the same path. That
//! determinism is what makes the rename corpus (`corpus/naming/*.toml`) a usable
//! spec and what lets the differential oracle diff us against the originals.
//!
//! ## Format grammar
//! A format is a template string with `{Token Name}` placeholders. A literal
//! brace is written `{{` / `}}`. Path separators (`/`) in the format are honored
//! as directory boundaries; each rendered *segment* between separators is
//! sanitized independently so a token value containing `/` cannot escape its
//! segment and create unexpected directories.
//!
//! ## Sanitization (per platform)
//! Filesystems disagree on what bytes are legal in a name. We sanitize for the
//! *most restrictive* target we support (Windows/exFAT) by default so a library
//! authored on Linux still moves cleanly to a Windows share — the originals
//! learned this the hard way. Reserved characters (`< > : " | ? *` and the
//! control range), reserved device names (`CON`, `PRN`, …), and trailing dots or
//! spaces are all handled.

use cellarr_core::NamingTokens;

use crate::error::{FsError, Result};

/// Characters that are illegal in a path *segment* on at least one supported
/// platform. We strip the union so names are portable across Linux, macOS, and
/// Windows/SMB shares. The forward slash is handled separately as a segment
/// boundary, never as a sanitizable character within a segment.
const ILLEGAL_SEGMENT_CHARS: &[char] = &['<', '>', ':', '"', '\\', '|', '?', '*'];

/// Windows reserves these device names regardless of extension. A segment equal
/// to one of these (case-insensitively, ignoring any extension) gets a marker
/// appended so it remains a legal, non-colliding name.
const RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// The replacement used when an illegal character is removed mid-word and we
/// would otherwise collapse two words together. Chosen to match the originals'
/// default of preserving readability without introducing new illegal characters.
const SPACE: char = ' ';

/// Render a naming `format` against `tokens`, producing a sanitized relative
/// path.
///
/// The returned string uses `/` separators (the caller joins it onto a root).
/// Each segment is sanitized independently; empty segments (e.g. a token that
/// rendered to nothing, leaving `//`) are dropped so the path never contains a
/// surprise empty directory.
///
/// # Errors
/// - [`FsError::MissingToken`] if the format references a token the module did
///   not supply (we refuse to silently produce a misnamed file).
/// - [`FsError::InvalidName`] if, after rendering and sanitizing, the result is
///   empty (no usable name at all).
pub fn render_name(format: &str, tokens: &NamingTokens) -> Result<String> {
    let rendered = substitute(format, tokens)?;

    let segments: Vec<String> = rendered
        .split('/')
        .map(sanitize_segment)
        .filter(|s| !s.is_empty())
        .collect();

    if segments.is_empty() {
        return Err(FsError::InvalidName {
            detail: format!("format {format:?} produced an empty path"),
        });
    }

    Ok(segments.join("/"))
}

/// Substitute `{Token}` placeholders. Supports `{{`/`}}` for literal braces.
fn substitute(format: &str, tokens: &NamingTokens) -> Result<String> {
    let mut out = String::with_capacity(format.len());
    let mut chars = format.char_indices().peekable();

    while let Some((_, c)) = chars.next() {
        match c {
            '{' => {
                if matches!(chars.peek(), Some((_, '{'))) {
                    chars.next();
                    out.push('{');
                    continue;
                }
                let mut name = String::new();
                let mut closed = false;
                for (_, nc) in chars.by_ref() {
                    if nc == '}' {
                        closed = true;
                        break;
                    }
                    name.push(nc);
                }
                if !closed {
                    return Err(FsError::InvalidName {
                        detail: format!("unterminated token in format {format:?}"),
                    });
                }
                let value = lookup(tokens, name.trim()).ok_or_else(|| FsError::MissingToken {
                    token: name.trim().to_string(),
                })?;
                // A slash inside a *token value* (e.g. an artist "AC/DC") must
                // not create a new directory level — only slashes written in the
                // format itself are structural. Neutralize value slashes here so
                // segment splitting downstream sees only format separators.
                if value.contains('/') {
                    out.push_str(&value.replace('/', " "));
                } else {
                    out.push_str(value);
                }
            }
            '}' => {
                if matches!(chars.peek(), Some((_, '}'))) {
                    chars.next();
                    out.push('}');
                } else {
                    return Err(FsError::InvalidName {
                        detail: format!("unescaped '}}' in format {format:?}"),
                    });
                }
            }
            _ => out.push(c),
        }
    }

    Ok(out)
}

/// Look up a token by name. The lookup is exact (token names come from the media
/// module's own vocabulary, not free text), so a typo in a format surfaces as a
/// [`FsError::MissingToken`] rather than silently rendering nothing.
fn lookup<'a>(tokens: &'a NamingTokens, name: &str) -> Option<&'a str> {
    tokens
        .tokens
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Sanitize a single path segment for portability across all supported
/// platforms. Returns an empty string for segments that reduce to nothing so the
/// caller can drop them.
fn sanitize_segment(segment: &str) -> String {
    // Replace illegal characters and control codes. We replace (rather than
    // delete) with a space so "A:B" becomes "A B" instead of "AB", preserving
    // word boundaries — the behavior the originals settled on.
    let mut cleaned: String = segment
        .chars()
        .map(|c| {
            if ILLEGAL_SEGMENT_CHARS.contains(&c) || c.is_control() {
                SPACE
            } else {
                c
            }
        })
        .collect();

    // Collapse runs of whitespace introduced by replacement, then trim.
    cleaned = collapse_whitespace(&cleaned);

    // Windows forbids trailing dots and spaces on a name; strip them. A name
    // that was *only* dots/spaces collapses to empty and is dropped upstream.
    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c == ' ');
    let mut result = trimmed.to_string();

    if is_reserved_device_name(&result) {
        result.push('_');
    }

    result
}

/// Collapse internal runs of spaces/tabs to a single space and trim the ends.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        let is_space = c == ' ' || c == '\t';
        if is_space {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Whether a segment matches a Windows reserved device name, ignoring case and
/// any file extension (`CON.txt` is just as reserved as `CON`).
fn is_reserved_device_name(segment: &str) -> bool {
    let stem = segment.split('.').next().unwrap_or(segment);
    RESERVED_DEVICE_NAMES
        .iter()
        .any(|r| r.eq_ignore_ascii_case(stem))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tk(pairs: &[(&str, &str)]) -> NamingTokens {
        NamingTokens {
            tokens: pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    #[test]
    fn substitutes_simple_tokens() {
        let t = tk(&[("Series Title", "The Show"), ("Season", "02")]);
        let out = render_name("{Series Title}/Season {Season}", &t).unwrap();
        assert_eq!(out, "The Show/Season 02");
    }

    #[test]
    fn missing_token_is_an_error_not_a_blank() {
        let t = tk(&[("Series Title", "The Show")]);
        let err = render_name("{Series Title} - {Episode Title}", &t).unwrap_err();
        assert!(matches!(err, FsError::MissingToken { token } if token == "Episode Title"));
    }

    #[test]
    fn illegal_characters_become_spaces_preserving_word_boundaries() {
        let t = tk(&[("Title", "Quo Vadis: Aida?")]);
        let out = render_name("{Title}", &t).unwrap();
        // ':' and '?' are illegal -> spaces -> collapsed/trimmed.
        assert_eq!(out, "Quo Vadis Aida");
    }

    #[test]
    fn token_value_with_slash_cannot_escape_its_segment() {
        // A title containing a slash must not create a new directory level.
        let t = tk(&[("Title", "AC/DC Live")]);
        let out = render_name("Music/{Title}", &t).unwrap();
        // The value's slash is neutralized to a space; only the format's slash
        // is structural, so this is exactly two segments.
        assert_eq!(out, "Music/AC DC Live");
    }

    #[test]
    fn literal_braces_are_escaped() {
        let t = tk(&[("Title", "Brackets")]);
        let out = render_name("{{not a token}} {Title}", &t).unwrap();
        assert_eq!(out, "{not a token} Brackets");
    }

    #[test]
    fn reserved_device_names_are_disambiguated() {
        let t = tk(&[("Title", "CON")]);
        let out = render_name("{Title}", &t).unwrap();
        assert_eq!(out, "CON_");
        let t2 = tk(&[("Title", "nul.mkv")]);
        let out2 = render_name("{Title}", &t2).unwrap();
        assert_eq!(out2, "nul.mkv_");
    }

    #[test]
    fn trailing_dots_and_spaces_are_stripped() {
        let t = tk(&[("Title", "Movie Title. ")]);
        let out = render_name("{Title}", &t).unwrap();
        assert_eq!(out, "Movie Title");
    }

    #[test]
    fn unicode_is_preserved() {
        let t = tk(&[("Title", "Amélie 北京 Ω")]);
        let out = render_name("{Title}", &t).unwrap();
        assert_eq!(out, "Amélie 北京 Ω");
    }

    #[test]
    fn empty_render_is_an_error() {
        let t = tk(&[("Title", "   ")]);
        let err = render_name("{Title}", &t).unwrap_err();
        assert!(matches!(err, FsError::InvalidName { .. }));
    }

    #[test]
    fn deterministic_same_inputs_same_output() {
        let t = tk(&[("A", "x"), ("B", "y")]);
        let a = render_name("{A}/{B}", &t).unwrap();
        let b = render_name("{A}/{B}", &t).unwrap();
        assert_eq!(a, b);
    }
}
