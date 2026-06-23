//! Property test: the parser never panics on arbitrary input, and is
//! deterministic (same input → same output).
//!
//! The parser is reachable from untrusted indexer/file names, so a panic would
//! be a denial of service. This pins the "never panics" non-negotiable from the
//! spec across a wide space of adversarial strings, including dense punctuation,
//! many digits, brackets, and unicode.

use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn never_panics_on_arbitrary_unicode(s in ".{0,200}") {
        let _ = cellarr_parse::parse_title(&s);
    }

    #[test]
    fn never_panics_on_release_like_soup(
        s in proptest::collection::vec(
            prop_oneof![
                Just("S01E01".to_string()),
                Just("1080p".to_string()),
                Just("WEB-DL".to_string()),
                Just("x264".to_string()),
                Just("-".to_string()),
                Just(".".to_string()),
                Just("[".to_string()),
                Just("]".to_string()),
                "[a-zA-Z0-9]{1,6}",
                "[0-9]{1,6}",
            ],
            0..40,
        ).prop_map(|v| v.join(""))
    ) {
        let _ = cellarr_parse::parse_title(&s);
    }

    #[test]
    fn deterministic(s in ".{0,200}") {
        let a = cellarr_parse::parse_title(&s);
        let b = cellarr_parse::parse_title(&s);
        prop_assert_eq!(a, b);
    }
}
