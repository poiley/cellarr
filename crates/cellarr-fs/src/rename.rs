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
//! ### Format specifiers (padding)
//! A token may carry a colon-introduced format specifier of zero-pad digits, e.g.
//! `{Season:00}`, `{episode:00}`, `{absolute:000}`. When the token's value is a
//! plain integer it is left-padded with zeros to the requested width; a
//! non-numeric value (or a value that already exceeds the width) is emitted
//! verbatim. This is how a module can supply raw numbers (`Season = "2"`) and let
//! the *format* decide the on-disk padding, matching the originals' `{season:00}`
//! convention.
//!
//! ### The leading-dash group token
//! A token name written with a leading dash — `{-Release Group}` — renders as a
//! ` -GroupName` suffix *only when the group is present and non-empty*, and
//! collapses to nothing when it is absent. This mirrors the originals' optional
//! `{-Release Group}` token so a release with no group does not leave a dangling
//! ` -` in the name.
//!
//! ## Multi-episode style
//! A single file can cover several consecutive episodes. The
//! [`MultiEpisodeStyle`] controls how the season/episode block for such a file is
//! rendered (`S01E01-E03`, `E01E02E03`, `S01E01.S01E02`, …). The engine derives
//! the block from the *raw* `Season` + `Episodes` tokens (`Episodes` is a
//! comma-separated list of episode numbers) so the module never has to know the
//! user's chosen style — it just lists the episodes the file contains.
//!
//! ## Sanitization (per platform)
//! Filesystems disagree on what bytes are legal in a name. By default we sanitize
//! for the *most restrictive* target we support (Windows/exFAT) so a library
//! authored on Linux still moves cleanly to a Windows share — the originals
//! learned this the hard way. Reserved characters (`< > : " | ? *` and the
//! control range), reserved device names (`CON`, `PRN`, …), and trailing dots or
//! spaces are all handled. [`RenderOptions`] tunes the colon replacement and the
//! target platform.

use cellarr_core::NamingTokens;

use crate::error::{FsError, Result};

/// Characters that are illegal in a path *segment* on at least one supported
/// platform, excluding the colon (handled separately so its replacement is
/// configurable). The forward slash is handled as a segment boundary, never as a
/// sanitizable character within a segment.
const ILLEGAL_SEGMENT_CHARS: &[char] = &['<', '>', '"', '\\', '|', '?', '*'];

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

/// How a colon (`:`) in a token *value* is rewritten when sanitizing for a
/// platform that forbids it (Windows/macOS-SMB). The colon is special-cased
/// because the originals expose it as a user-tunable "colon replacement" setting:
/// some libraries want `Title - Subtitle`, others `Title Subtitle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColonReplacement {
    /// `A: B` → `A B` — replace the colon with a single space (then collapse).
    /// The originals' default and ours.
    #[default]
    Space,
    /// `A: B` → `A - B` — replace the colon (with surrounding spaces normalized)
    /// with a spaced dash, preserving the visual separation.
    Dash,
    /// `A: B` → `AB` — delete the colon outright (then collapse spaces).
    Delete,
    /// `A: B` → `A- B` — Smart: a colon flanked by spaces becomes ` - `, a colon
    /// with no trailing space becomes `-`. Mirrors the originals' "Smart" mode.
    Smart,
    /// Keep the colon verbatim. Valid only when the target platform permits it
    /// (Linux); on a colon-hostile platform this falls back to [`Space`].
    Keep,
}

/// The most-restrictive platform the rendered path must remain legal on.
///
/// Naming is sanitized for this target. The default — [`Windows`] — is the
/// strictest and keeps a library portable to an SMB/exFAT share even when authored
/// on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetPlatform {
    /// Windows/exFAT/SMB — forbids `< > : " \ | ? *`, control codes, reserved
    /// device names, and trailing dots/spaces. The portable default.
    #[default]
    Windows,
    /// POSIX/Linux — only `/` and NUL are truly illegal. The colon is permitted,
    /// so [`ColonReplacement::Keep`] is honored here.
    Posix,
}

impl TargetPlatform {
    /// Whether a literal colon is legal in a name on this platform.
    fn allows_colon(self) -> bool {
        matches!(self, TargetPlatform::Posix)
    }
}

/// How a file that covers several consecutive episodes renders its season/episode
/// block. The default is [`PrefixedRange`](MultiEpisodeStyle::PrefixedRange), the
/// TRaSH-recommended style.
///
/// Examples below assume season 1, episodes 1–3, padded to two digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MultiEpisodeStyle {
    /// `S01E01-E03` — the season block once, then the episode range with an `E`
    /// prefix on the last episode. The default (TRaSH).
    #[default]
    PrefixedRange,
    /// `S01E01-03` — the range without the trailing `E` prefix.
    Range,
    /// `S01E01E02E03` — every episode listed, each with its own `E` prefix.
    Extend,
    /// `S01E01.S01E02.S01E03` — the full `SxxExx` block duplicated per episode,
    /// dot-joined.
    Duplicate,
    /// `S01E01E02E03` — like Extend; the season appears once and each episode is
    /// repeated with an `E` prefix. (Distinct config knob; same shape as Extend
    /// for contiguous runs.)
    Repeat,
    /// `S01E01-E02-E03` — scene-style, every adjacent pair joined by `-E`.
    Scene,
}

/// Tunables for [`render_name_with`]. The defaults reproduce [`render_name`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderOptions {
    /// The strictest platform the result must stay legal on.
    pub platform: TargetPlatform,
    /// How a colon in a token value is rewritten when the platform forbids it.
    pub colon: ColonReplacement,
    /// How a multi-episode block is rendered.
    pub multi_episode: MultiEpisodeStyle,
}

/// Render a naming `format` against `tokens` with the default options.
///
/// Equivalent to [`render_name_with`] with [`RenderOptions::default`] (portable
/// Windows-safe sanitization, space colon-replacement, PrefixedRange multi-ep).
///
/// # Errors
/// See [`render_name_with`].
pub fn render_name(format: &str, tokens: &NamingTokens) -> Result<String> {
    render_name_with(format, tokens, RenderOptions::default())
}

/// Render a naming `format` against `tokens`, producing a sanitized relative path.
///
/// The returned string uses `/` separators (the caller joins it onto a root).
/// Each segment is sanitized independently; empty segments (e.g. a token that
/// rendered to nothing, leaving `//`) are dropped so the path never contains a
/// surprise empty directory.
///
/// # Errors
/// - [`FsError::MissingToken`] if the format references a *required* token the
///   module did not supply (we refuse to silently produce a misnamed file). The
///   optional `{-Release Group}` token and the enrichment tokens listed by
///   [`is_optional_token`] (e.g. `{Release Year}`, `{Edition}`) are exempt: they
///   render to nothing when absent, and any bracket/paren group left empty by the
///   dropped token (`({Release Year})`) is cleaned up, so a movie with no known
///   year still renders a valid `Movie Title/Movie Title.ext`.
/// - [`FsError::InvalidName`] if, after rendering and sanitizing, the result is
///   empty (no usable name at all), or the format is malformed (unterminated
///   token, stray brace).
pub fn render_name_with(
    format: &str,
    tokens: &NamingTokens,
    opts: RenderOptions,
) -> Result<String> {
    let rendered = substitute(format, tokens, opts)?;

    let segments: Vec<String> = rendered
        .split('/')
        .map(|seg| sanitize_segment(seg, opts))
        .filter(|s| !s.is_empty())
        .collect();

    if segments.is_empty() {
        return Err(FsError::InvalidName {
            detail: format!("format {format:?} produced an empty path"),
        });
    }

    Ok(segments.join("/"))
}

/// Substitute `{Token}` placeholders. Supports `{{`/`}}` for literal braces, a
/// `:spec` zero-pad specifier, the optional `{-Token}` suffix form, and the
/// engine-computed `{Episode Block}` multi-episode token.
fn substitute(format: &str, tokens: &NamingTokens, opts: RenderOptions) -> Result<String> {
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
                render_token(&mut out, name.trim(), tokens, opts)?;
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

/// Resolve and append a single `{...}` token's value to `out`.
fn render_token(
    out: &mut String,
    raw: &str,
    tokens: &NamingTokens,
    opts: RenderOptions,
) -> Result<()> {
    // Split an optional `:spec` format specifier (zero-pad width) off the name.
    let (name_part, spec) = match raw.split_once(':') {
        Some((n, s)) => (n.trim(), Some(s.trim())),
        None => (raw, None),
    };

    // The engine-computed multi-episode block: built from raw Season + Episodes
    // per the configured style, not looked up as a single value.
    if name_part.eq_ignore_ascii_case("Episode Block") {
        let block = render_episode_block(tokens, spec, opts)?;
        out.push_str(&block);
        return Ok(());
    }

    // The optional leading-dash group token: ` -Group` when present, else nothing.
    if let Some(group_name) = name_part.strip_prefix('-') {
        let group_name = group_name.trim();
        if let Some(value) = lookup(tokens, group_name) {
            let value = value.trim();
            if !value.is_empty() {
                out.push_str(" -");
                push_value(out, value);
            }
        }
        // Absent or empty -> render nothing. Never an error: it is optional.
        return Ok(());
    }

    let Some(value) = lookup(tokens, name_part) else {
        // An *optional* token the module did not supply (a movie with no known
        // release year, an absent edition) renders to nothing rather than failing
        // the whole import — the surrounding bracket/paren group, if any, is
        // cleaned up during sanitization so no dangling `()` is left. A *required*
        // token still hard-errors: refusing to silently produce a misnamed file
        // is the core safety property (a missing title/extension is never
        // optional).
        if is_optional_token(name_part) {
            return Ok(());
        }
        return Err(FsError::MissingToken {
            token: name_part.to_string(),
        });
    };

    let formatted = apply_spec(value, spec);
    push_value(out, &formatted);
    Ok(())
}

/// Whether a token is *optional* — its absence renders to nothing instead of
/// erroring. These are the enrichment tokens the metadata source may legitimately
/// not know (a movie with no looked-up release year, an absent edition tag,
/// best-effort MediaInfo, external ids), where dropping the token (and any
/// now-empty surrounding `()`/`[]`/`{}` group) yields a still-valid name. The
/// structural tokens a name cannot do without — title and extension — are
/// deliberately *not* listed: their absence is a real fault that must surface.
fn is_optional_token(name: &str) -> bool {
    const OPTIONAL: &[&str] = &[
        "Release Year",
        "Year",
        "Edition",
        "Edition Tags",
        "Custom Formats",
        "MediaInfo VideoCodec",
        "MediaInfo AudioCodec",
        "MediaInfo AudioChannels",
        "ImdbId",
        "TmdbId",
        "TvdbId",
        "Absolute Episode",
    ];
    OPTIONAL.iter().any(|o| o.eq_ignore_ascii_case(name))
}

/// Push a token value into the output, neutralizing any `/` so a value cannot
/// create a directory level (only format separators are structural).
fn push_value(out: &mut String, value: &str) {
    if value.contains('/') {
        out.push_str(&value.replace('/', " "));
    } else {
        out.push_str(value);
    }
}

/// Apply a `:spec` format specifier to a value. The only spec we honor is a run
/// of zero-pad digits (`00`, `000`): left-pad an integer value to that width. A
/// non-numeric value, or one already at/over the width, is returned verbatim.
fn apply_spec(value: &str, spec: Option<&str>) -> String {
    let Some(spec) = spec else {
        return value.to_string();
    };
    let width = zero_pad_width(spec);
    let Some(width) = width else {
        // Unrecognized spec: emit the value untouched rather than guess.
        return value.to_string();
    };
    pad_number(value, width)
}

/// Interpret a spec as a zero-pad width. `"00"` -> 2, `"000"` -> 3. Anything that
/// is not a run of `0`s (e.g. an empty spec or arbitrary text) is rejected.
fn zero_pad_width(spec: &str) -> Option<usize> {
    if !spec.is_empty() && spec.chars().all(|c| c == '0') {
        Some(spec.len())
    } else {
        None
    }
}

/// Left-pad a value with zeros to `width` *iff* it parses as a non-negative
/// integer; otherwise return it unchanged.
fn pad_number(value: &str, width: usize) -> String {
    match value.trim().parse::<u64>() {
        Ok(n) => format!("{n:0width$}"),
        Err(_) => value.to_string(),
    }
}

/// Build the season/episode block for a (possibly multi-)episode file from the
/// raw `Season` + `Episodes` tokens and the configured [`MultiEpisodeStyle`].
///
/// `Season` is required. `Episodes` is a comma-separated list of episode numbers
/// (`"1"`, `"1,2,3"`); a single `Episode` token is accepted as a fallback for the
/// single-episode case. The `spec` (e.g. `00`) zero-pads both the season and each
/// episode number consistently.
fn render_episode_block(
    tokens: &NamingTokens,
    spec: Option<&str>,
    opts: RenderOptions,
) -> Result<String> {
    let season = lookup(tokens, "Season").ok_or_else(|| FsError::MissingToken {
        token: "Season".to_string(),
    })?;

    let episodes_raw = lookup(tokens, "Episodes")
        .or_else(|| lookup(tokens, "Episode"))
        .ok_or_else(|| FsError::MissingToken {
            token: "Episodes".to_string(),
        })?;

    let episodes: Vec<String> = episodes_raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|e| pad_number(e, zero_pad_width(spec.unwrap_or("")).unwrap_or(0)))
        .collect();

    if episodes.is_empty() {
        return Err(FsError::InvalidName {
            detail: "Episodes token held no episode numbers".to_string(),
        });
    }

    let season = pad_number(season, zero_pad_width(spec.unwrap_or("")).unwrap_or(0));
    let s = format!("S{season}");

    // Single episode: every style reduces to SxxEyy.
    if episodes.len() == 1 {
        return Ok(format!("{s}E{}", episodes[0]));
    }

    let first = &episodes[0];
    let last = &episodes[episodes.len() - 1];

    let block = match opts.multi_episode {
        MultiEpisodeStyle::PrefixedRange => format!("{s}E{first}-E{last}"),
        MultiEpisodeStyle::Range => format!("{s}E{first}-{last}"),
        MultiEpisodeStyle::Extend | MultiEpisodeStyle::Repeat => {
            let mut b = s.clone();
            for e in &episodes {
                b.push('E');
                b.push_str(e);
            }
            b
        }
        MultiEpisodeStyle::Duplicate => episodes
            .iter()
            .map(|e| format!("{s}E{e}"))
            .collect::<Vec<_>>()
            .join("."),
        MultiEpisodeStyle::Scene => {
            let mut b = format!("{s}E{first}");
            for e in &episodes[1..] {
                b.push_str("-E");
                b.push_str(e);
            }
            b
        }
    };

    Ok(block)
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

/// Sanitize a single path segment for portability across the target platform.
/// Returns an empty string for segments that reduce to nothing so the caller can
/// drop them.
fn sanitize_segment(segment: &str, opts: RenderOptions) -> String {
    // The colon is handled first, with its configurable replacement, *before* the
    // generic illegal-character pass so a Dash/Smart replacement can insert its
    // own dash without that dash being seen as illegal.
    let colon_handled = replace_colons(segment, opts);

    let mut cleaned: String = colon_handled
        .chars()
        .map(|c| {
            if ILLEGAL_SEGMENT_CHARS.contains(&c) || c.is_control() {
                SPACE
            } else {
                c
            }
        })
        .collect();

    // Drop bracket/paren groups left empty by a dropped optional token (an
    // identified movie with no known year renders `{Movie Title} ({Release Year})`
    // → `Movie Title ()` → `Movie Title`), so the name carries no dangling `()`.
    cleaned = drop_empty_bracket_groups(&cleaned);

    // Collapse runs of whitespace introduced by replacement, then trim.
    cleaned = collapse_whitespace(&cleaned);

    // Windows forbids trailing dots and spaces on a name; strip them. A name that
    // was *only* dots/spaces collapses to empty and is dropped upstream.
    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c == ' ');
    let mut result = trimmed.to_string();

    if is_reserved_device_name(&result) {
        result.push('_');
    }

    result
}

/// Rewrite colons in a segment per the configured [`ColonReplacement`], honoring
/// the platform (a colon-tolerant platform with [`ColonReplacement::Keep`] leaves
/// them be).
fn replace_colons(segment: &str, opts: RenderOptions) -> String {
    if !segment.contains(':') {
        return segment.to_string();
    }

    let keep = matches!(opts.colon, ColonReplacement::Keep) && opts.platform.allows_colon();
    if keep {
        return segment.to_string();
    }

    match opts.colon {
        ColonReplacement::Delete => segment.replace(':', ""),
        ColonReplacement::Dash => {
            // Normalize ` : ` / `: ` / `:` to a spaced dash; whitespace is
            // collapsed downstream so a single rule suffices.
            segment.replace(':', " - ")
        }
        ColonReplacement::Smart => {
            // ` : ` -> ` - `, but `:` with no trailing space -> `-` (e.g. ratios
            // like "16:9" stay tight: "16-9").
            let mut out = String::with_capacity(segment.len());
            let chars: Vec<char> = segment.chars().collect();
            for (i, &c) in chars.iter().enumerate() {
                if c == ':' {
                    let next_is_space =
                        chars.get(i + 1).map(|n| n.is_whitespace()).unwrap_or(false);
                    if next_is_space {
                        out.push_str(" -");
                    } else {
                        out.push('-');
                    }
                } else {
                    out.push(c);
                }
            }
            out
        }
        // Space (and Keep on a colon-hostile platform) -> a single space.
        ColonReplacement::Space | ColonReplacement::Keep => segment.replace(':', " "),
    }
}

/// Remove bracket/paren groups that hold only whitespace — `()`, `[]`, `{}`,
/// `(  )` — left behind when an optional token inside them rendered to nothing
/// (e.g. `{Movie Title} ({Release Year})` for a movie with no known year). The
/// matched pair *and* its empty interior are dropped; non-empty groups and
/// unbalanced stray brackets are left untouched so a real `(2017)` or a literal
/// brace from `{{…}}` is never disturbed. Whitespace introduced/left around the
/// removed group is collapsed by the caller.
fn drop_empty_bracket_groups(s: &str) -> String {
    const PAIRS: &[(char, char)] = &[('(', ')'), ('[', ']'), ('{', '}')];
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some((_, close)) = PAIRS.iter().find(|(open, _)| *open == c) {
            // Scan past any whitespace to see if the matching close follows
            // immediately (an empty group). If so, skip the whole group.
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && chars[j] == *close {
                i = j + 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
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
        let t = tk(&[("Title", "AC/DC Live")]);
        let out = render_name("Music/{Title}", &t).unwrap();
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

    // --- format specifiers (padding) ---

    #[test]
    fn zero_pad_specifier_pads_numbers() {
        let t = tk(&[("Season", "2"), ("Episode", "6"), ("Absolute", "71")]);
        let out = render_name("S{Season:00}E{Episode:00} {Absolute:000}", &t).unwrap();
        assert_eq!(out, "S02E06 071");
    }

    #[test]
    fn zero_pad_leaves_non_numeric_and_wider_values_alone() {
        let t = tk(&[("Episode", "123")]);
        // already 3 digits, width 2 -> unchanged
        assert_eq!(render_name("{Episode:00}", &t).unwrap(), "123");
        let t2 = tk(&[("Tag", "Final")]);
        assert_eq!(render_name("{Tag:00}", &t2).unwrap(), "Final");
    }

    #[test]
    fn unknown_specifier_is_ignored() {
        let t = tk(&[("X", "value")]);
        assert_eq!(render_name("{X:weird}", &t).unwrap(), "value");
    }

    #[test]
    fn non_zero_specifier_does_not_zero_pad_a_numeric_value() {
        // A spec is a zero-pad width ONLY when it is a run of `0`s. A non-`0`
        // spec (here "77") must be rejected and the value emitted verbatim — it
        // must NOT be misread as "width 2". With a NUMERIC value this is the case
        // that distinguishes the real `&& all-zeros` rule from an `|| all-zeros`
        // relaxation, which would pad "5" to "05".
        let t = tk(&[("N", "5")]);
        assert_eq!(render_name("{N:77}", &t).unwrap(), "5");
        // And an all-zeros spec of the same width DOES pad, proving the width is
        // honored when the spec is legitimate.
        let t2 = tk(&[("N", "5")]);
        assert_eq!(render_name("{N:00}", &t2).unwrap(), "05");
    }

    // --- leading-dash optional group ---

    #[test]
    fn dash_group_renders_when_present() {
        let t = tk(&[("Title", "Movie"), ("Release Group", "NTb")]);
        let out = render_name("{Title}{-Release Group}", &t).unwrap();
        assert_eq!(out, "Movie -NTb");
    }

    #[test]
    fn dash_group_collapses_when_absent() {
        let t = tk(&[("Title", "Movie")]);
        let out = render_name("{Title}{-Release Group}", &t).unwrap();
        assert_eq!(out, "Movie");
    }

    #[test]
    fn dash_group_collapses_when_empty() {
        let t = tk(&[("Title", "Movie"), ("Release Group", "")]);
        let out = render_name("{Title}{-Release Group}", &t).unwrap();
        assert_eq!(out, "Movie");
    }

    // --- optional tokens (graceful absence) ---

    #[test]
    fn movie_with_year_renders_title_year() {
        let t = tk(&[("Movie Title", "Blade Runner"), ("Release Year", "1982")]);
        let out = render_name(
            "{Movie Title} ({Release Year})/{Movie Title}.{Extension}",
            &tk(&[
                ("Movie Title", "Blade Runner"),
                ("Release Year", "1982"),
                ("Extension", "mkv"),
            ]),
        )
        .unwrap();
        assert_eq!(out, "Blade Runner (1982)/Blade Runner.mkv");
        let _ = t;
    }

    #[test]
    fn missing_optional_year_drops_token_and_empty_parens() {
        // A movie whose year is unknown: the optional {Release Year} renders to
        // nothing and the now-empty `()` is cleaned up, so the import still lands.
        let t = tk(&[("Movie Title", "Unknown Movie"), ("Extension", "mkv")]);
        let out = render_name(
            "{Movie Title} ({Release Year})/{Movie Title}.{Extension}",
            &t,
        )
        .unwrap();
        assert_eq!(out, "Unknown Movie/Unknown Movie.mkv");
    }

    #[test]
    fn missing_required_token_still_errors() {
        // The extension is structural, never optional: its absence must surface.
        let t = tk(&[("Movie Title", "Some Movie")]);
        let err = render_name("{Movie Title}.{Extension}", &t).unwrap_err();
        assert!(matches!(err, FsError::MissingToken { token } if token == "Extension"));
    }

    #[test]
    fn empty_bracket_groups_are_dropped_but_real_ones_kept() {
        assert_eq!(drop_empty_bracket_groups("Movie ()"), "Movie ");
        assert_eq!(drop_empty_bracket_groups("Movie [ ]"), "Movie ");
        assert_eq!(drop_empty_bracket_groups("Movie (2017)"), "Movie (2017)");
        // an unbalanced stray bracket is left alone
        assert_eq!(drop_empty_bracket_groups("Movie ("), "Movie (");
    }

    // --- multi-episode styles ---

    fn ep_tokens() -> NamingTokens {
        tk(&[("Season", "1"), ("Episodes", "1,2,3")])
    }

    fn render_block(style: MultiEpisodeStyle) -> String {
        let opts = RenderOptions {
            multi_episode: style,
            ..Default::default()
        };
        render_name_with("{Episode Block:00}", &ep_tokens(), opts).unwrap()
    }

    #[test]
    fn multi_ep_prefixed_range_is_default() {
        assert_eq!(render_block(MultiEpisodeStyle::PrefixedRange), "S01E01-E03");
        // default options also yield PrefixedRange
        assert_eq!(
            render_name("{Episode Block:00}", &ep_tokens()).unwrap(),
            "S01E01-E03"
        );
    }

    #[test]
    fn multi_ep_range() {
        assert_eq!(render_block(MultiEpisodeStyle::Range), "S01E01-03");
    }

    #[test]
    fn multi_ep_extend_and_repeat() {
        assert_eq!(render_block(MultiEpisodeStyle::Extend), "S01E01E02E03");
        assert_eq!(render_block(MultiEpisodeStyle::Repeat), "S01E01E02E03");
    }

    #[test]
    fn multi_ep_duplicate() {
        assert_eq!(
            render_block(MultiEpisodeStyle::Duplicate),
            "S01E01.S01E02.S01E03"
        );
    }

    #[test]
    fn multi_ep_scene() {
        assert_eq!(render_block(MultiEpisodeStyle::Scene), "S01E01-E02-E03");
    }

    #[test]
    fn single_episode_block_reduces_to_sxxeyy_for_every_style() {
        let t = tk(&[("Season", "2"), ("Episodes", "6")]);
        for style in [
            MultiEpisodeStyle::PrefixedRange,
            MultiEpisodeStyle::Range,
            MultiEpisodeStyle::Extend,
            MultiEpisodeStyle::Repeat,
            MultiEpisodeStyle::Duplicate,
            MultiEpisodeStyle::Scene,
        ] {
            let opts = RenderOptions {
                multi_episode: style,
                ..Default::default()
            };
            assert_eq!(
                render_name_with("{Episode Block:00}", &t, opts).unwrap(),
                "S02E06"
            );
        }
    }

    #[test]
    fn episode_block_uses_single_episode_token_as_fallback() {
        // No Episodes/Episode token at all -> MissingToken.
        let bare = tk(&[("Season", "3")]);
        let err = render_name("{Episode Block:00}", &bare).unwrap_err();
        assert!(matches!(err, FsError::MissingToken { token } if token == "Episodes"));

        // The single `Episode` token is accepted as a fallback for `Episodes`.
        let t = tk(&[("Season", "3"), ("Episode", "14")]);
        assert_eq!(render_name("{Episode Block:00}", &t).unwrap(), "S03E14");
    }

    // --- colon replacement modes ---

    fn render_colon(colon: ColonReplacement, platform: TargetPlatform, value: &str) -> String {
        let opts = RenderOptions {
            colon,
            platform,
            ..Default::default()
        };
        render_name_with("{Title}", &tk(&[("Title", value)]), opts).unwrap()
    }

    #[test]
    fn colon_space_is_default() {
        assert_eq!(
            render_colon(ColonReplacement::Space, TargetPlatform::Windows, "A: B"),
            "A B"
        );
    }

    #[test]
    fn colon_dash() {
        assert_eq!(
            render_colon(ColonReplacement::Dash, TargetPlatform::Windows, "A: B"),
            "A - B"
        );
    }

    #[test]
    fn colon_delete() {
        assert_eq!(
            render_colon(ColonReplacement::Delete, TargetPlatform::Windows, "A: B"),
            "A B"
        );
        // delete with no space leaves words joined
        assert_eq!(
            render_colon(ColonReplacement::Delete, TargetPlatform::Windows, "16:9"),
            "169"
        );
    }

    #[test]
    fn colon_smart_distinguishes_spaced_from_tight() {
        assert_eq!(
            render_colon(ColonReplacement::Smart, TargetPlatform::Windows, "A: B"),
            "A - B"
        );
        assert_eq!(
            render_colon(ColonReplacement::Smart, TargetPlatform::Windows, "16:9"),
            "16-9"
        );
    }

    #[test]
    fn colon_kept_on_posix_but_replaced_on_windows() {
        assert_eq!(
            render_colon(ColonReplacement::Keep, TargetPlatform::Posix, "A: B"),
            "A: B"
        );
        // Keep on a colon-hostile platform falls back to a space.
        assert_eq!(
            render_colon(ColonReplacement::Keep, TargetPlatform::Windows, "A: B"),
            "A B"
        );
    }
}
