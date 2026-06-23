//! Property tests for the rename engine [`render_name`] / [`render_name_with`].
//!
//! `render_name` turns module-supplied naming tokens + a user format into an
//! on-disk relative path. It is reachable with arbitrary token *values* (titles,
//! episode names, group names come from parsed, untrusted release/file names), so
//! it must be a total function with platform-safe, deterministic output. These
//! properties pin, over a wide random space:
//!
//! 1. **Never panics** on arbitrary token values and arbitrary (possibly
//!    malformed) format strings — a malformed format is an `Err`, never a crash.
//! 2. **No illegal characters** in a successful render for the target platform:
//!    on Windows the reserved set `< > : " \ | ? *`, control codes, and a token
//!    value's `/` must never survive into a path *segment*; trailing dots/spaces
//!    on a segment are stripped.
//! 3. **Deterministic**: the same tokens + format + options always render to the
//!    exact same string.
//! 4. **Idempotent sanitization**: feeding an already-rendered, already-sanitized
//!    segment back through render as a passthrough token value yields the same
//!    bytes (sanitizing a clean name is a no-op).

use cellarr_core::NamingTokens;
use cellarr_fs::{render_name, render_name_with, ColonReplacement, RenderOptions, TargetPlatform};
use proptest::prelude::*;

/// Characters that must never appear inside a rendered path *segment* on Windows
/// (the colon is included: with the default Space replacement it is rewritten).
const WINDOWS_ILLEGAL: &[char] = &['<', '>', ':', '"', '\\', '|', '?', '*'];

/// A strategy for token *values*: a spicy mix of legal text, reserved chars,
/// path separators, unicode, control bytes, and whitespace edges.
fn token_value() -> impl Strategy<Value = String> {
    prop_oneof![
        // Mostly-legal title-ish text.
        r"[A-Za-z0-9 ._-]{0,20}",
        // Adversarial: any printable+control ASCII soup (reserved chars, slashes).
        r"[\x00-\x7f]{0,20}",
        // Unicode + an embedded separator and trailing dot/space edges.
        Just("Amélie 北京/Ω. ".to_string()),
        Just("CON".to_string()),
        Just("Quo Vadis: Aida?".to_string()),
        Just("AC/DC".to_string()),
        Just(String::new()),
    ]
}

/// A token map: a Title plus a couple of optional extra tokens. The keys are
/// fixed (real token names come from a module's vocabulary, not free text) so the
/// format can reference them without spuriously hitting MissingToken.
fn tokens() -> impl Strategy<Value = NamingTokens> {
    (token_value(), token_value(), token_value()).prop_map(|(title, group, season)| NamingTokens {
        tokens: vec![
            ("Title".to_string(), title),
            ("Release Group".to_string(), group),
            ("Season".to_string(), season),
        ],
    })
}

/// A render-options strategy spanning both platforms, every colon mode.
fn options() -> impl Strategy<Value = RenderOptions> {
    let platform = prop_oneof![Just(TargetPlatform::Windows), Just(TargetPlatform::Posix)];
    let colon = prop_oneof![
        Just(ColonReplacement::Space),
        Just(ColonReplacement::Dash),
        Just(ColonReplacement::Delete),
        Just(ColonReplacement::Smart),
        Just(ColonReplacement::Keep),
    ];
    (platform, colon).prop_map(|(platform, colon)| RenderOptions {
        platform,
        colon,
        ..Default::default()
    })
}

/// Assert a successful Windows render carries no illegal characters in any
/// segment and no trailing dot/space on a segment.
fn assert_windows_clean(rendered: &str) {
    for segment in rendered.split('/') {
        for c in segment.chars() {
            assert!(
                !WINDOWS_ILLEGAL.contains(&c),
                "illegal char {c:?} survived into segment {segment:?} of {rendered:?}"
            );
            assert!(
                !c.is_control(),
                "control char {c:?} survived into {rendered:?}"
            );
        }
        // Segments are non-empty (empties are dropped) and have no trailing
        // dot/space (Windows forbids them).
        assert!(!segment.is_empty(), "empty segment in {rendered:?}");
        let last = segment.chars().last().unwrap();
        assert!(
            last != '.' && last != ' ',
            "segment {segment:?} ends in a dot/space in {rendered:?}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    /// render_name never panics on arbitrary token values and a passthrough
    /// format — it either renders or returns a structured Err (e.g. an empty
    /// result). Both are fine; a panic is not.
    #[test]
    fn render_never_panics_on_arbitrary_values(t in tokens(), opts in options()) {
        let _ = render_name_with("{Title}/{Season}{-Release Group}", &t, opts);
    }

    /// A successful Windows render is always platform-clean: no reserved chars,
    /// no control codes, no value-injected directory levels, no trailing dot/space.
    #[test]
    fn windows_render_emits_no_illegal_characters(t in tokens()) {
        let opts = RenderOptions {
            platform: TargetPlatform::Windows,
            ..Default::default()
        };
        if let Ok(rendered) = render_name_with("{Title}/{Season}", &t, opts) {
            assert_windows_clean(&rendered);
        }
    }

    /// A token value containing `/` can never create an extra directory level:
    /// the number of `/`-separated segments is bounded by the format's structural
    /// separators (here one), never inflated by a value.
    #[test]
    fn a_value_slash_cannot_create_directories(t in tokens()) {
        let opts = RenderOptions {
            platform: TargetPlatform::Windows,
            ..Default::default()
        };
        // Single-token, single-segment format: a successful render is exactly one
        // segment regardless of any `/` inside the Title value.
        if let Ok(rendered) = render_name_with("{Title}", &t, opts) {
            assert_eq!(
                rendered.split('/').count(),
                1,
                "a value's slash leaked a directory level: {rendered:?}"
            );
        }
    }

    /// Determinism: identical inputs always produce identical output.
    #[test]
    fn render_is_deterministic(t in tokens(), opts in options()) {
        let fmt = "{Title}/{Season}{-Release Group}";
        // FsError is not PartialEq; compare the Ok value and the err presence
        // separately so determinism is checked for both success and failure.
        let a = render_name_with(fmt, &t, opts);
        let b = render_name_with(fmt, &t, opts);
        prop_assert_eq!(a.is_ok(), b.is_ok());
        prop_assert_eq!(a.ok(), b.ok());
    }

    /// Idempotent sanitization: a value that has already been rendered+sanitized
    /// for a platform is a fixed point — feeding it back as a passthrough token
    /// value and rendering again yields the same bytes (sanitizing clean text is
    /// a no-op, so the rename engine never "drifts" a name on repeated passes).
    #[test]
    fn sanitization_is_idempotent(t in tokens(), opts in options()) {
        // First pass: render the Title alone to a sanitized segment.
        if let Ok(once) = render_name_with("{Title}", &t, opts) {
            // Second pass: feed the sanitized result straight back in as a value.
            let fed = NamingTokens {
                tokens: vec![("Title".to_string(), once.clone())],
            };
            let twice = render_name_with("{Title}", &fed, opts)
                .expect("a non-empty sanitized name re-renders");
            prop_assert_eq!(
                &once, &twice,
                "rendering an already-sanitized name changed it: {:?} -> {:?}",
                once, twice
            );
        }
    }

    /// The default-options render_name agrees with render_name_with(default) for
    /// any tokens — the convenience wrapper must not diverge from the explicit
    /// form (a regression here would silently change every user's library naming).
    #[test]
    fn render_name_matches_default_options(t in tokens()) {
        let fmt = "{Title}/Season {Season}{-Release Group}";
        // FsError is not PartialEq; compare the Ok values (both arms always
        // agree on success/failure since the wrapper just forwards defaults).
        let wrapper = render_name(fmt, &t);
        let explicit = render_name_with(fmt, &t, RenderOptions::default());
        prop_assert_eq!(wrapper.is_ok(), explicit.is_ok());
        prop_assert_eq!(wrapper.ok(), explicit.ok());
    }
}
