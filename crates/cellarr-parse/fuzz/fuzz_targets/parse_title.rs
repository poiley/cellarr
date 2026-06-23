//! libFuzzer target: `parse_title` must never panic on arbitrary input.
//!
//! Release titles arrive from untrusted indexers and file names, so a panic in
//! the parser would be a denial of service. This throws raw fuzzer-generated
//! bytes (interpreted as a UTF-8 title, lossily) at the deterministic fast path
//! and asserts total-function behavior: it returns a `ParsedRelease` for every
//! input, never unwinds.
//!
//! Run (nightly required):
//!   cargo +nightly fuzz run parse_title -- -max_total_time=60
//!
//! The stable-toolchain equivalent lives in `tests/proptest_no_panic.rs`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Interpret the fuzzer bytes as a (lossy) UTF-8 title so non-UTF-8 byte
    // sequences are still exercised through a valid &str boundary.
    let title = String::from_utf8_lossy(data);
    let parsed = cellarr_parse::parse_title(&title);
    // Determinism is part of the contract: a second parse of the same input must
    // be byte-identical. Cheap to check inline and catches any nondeterminism the
    // fuzzer surfaces.
    let again = cellarr_parse::parse_title(&title);
    assert_eq!(parsed, again, "parse_title is not deterministic for {title:?}");
});
