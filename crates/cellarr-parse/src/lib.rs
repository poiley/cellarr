//! cellarr-parse — the release-name parser.
//!
//! Turns a release/file title into a [`cellarr_core::ParsedRelease`] with
//! per-field confidence, via an extractor pipeline measured against the
//! `corpus/parse` vectors. See `docs/specs/cellarr-parse.md` and
//! `docs/04-parser.md`.
//!
//! The parser is a sequence of independent extractors (resolution, source,
//! codec, audio, hdr, edition, language, group, proper/repack, year, numbering).
//! Each contributes fields to the result with a confidence. Extraction is
//! deterministic and clean-room: implemented from scene/p2p/anime naming
//! conventions and the corpus, not transcribed from any upstream parser.
//!
//! The deterministic fast path never panics on arbitrary input (property
//! tested). Regexes are compiled once via [`std::sync::LazyLock`]; multi-pattern
//! phases use [`regex::RegexSet`]; lookaround is avoided in favour of multi-pass
//! extraction.

#![forbid(unsafe_code)]

mod audio;
mod codec;
mod edition;
mod group;
mod hdr;
mod language;
mod numbering;
mod proper_repack;
mod resolution;
mod source;
mod title;
mod tokens;
mod year;

use cellarr_core::parsed::{Confidence, ParsedField, ParsedRelease};
use rayon::prelude::*;

/// Parse a single release/file title into a [`ParsedRelease`].
///
/// This is the deterministic fast path. It never panics on arbitrary input.
#[must_use]
pub fn parse_title(input: &str) -> ParsedRelease {
    let mut out = ParsedRelease::new(input);

    // Numbering first: it identifies the boundary between the title text and the
    // tag soup, which the title extractor relies on for a clean title.
    numbering::extract(input, &mut out);
    year::extract(input, &mut out);
    resolution::extract(input, &mut out);
    source::extract(input, &mut out);
    codec::extract(input, &mut out);
    audio::extract(input, &mut out);
    hdr::extract(input, &mut out);
    edition::extract(input, &mut out);
    language::extract(input, &mut out);
    proper_repack::extract(input, &mut out);
    group::extract(input, &mut out);
    title::extract(input, &mut out);

    out
}

/// Parse many titles in parallel. Intended for search-time bursts.
#[must_use]
pub fn parse_batch(titles: &[&str]) -> Vec<ParsedRelease> {
    titles.par_iter().map(|t| parse_title(t)).collect()
}

/// The mean confidence over the populated fields, exposed so callers (and the
/// pipeline) can gate on a low-confidence parse before consulting the inference
/// fallback.
#[must_use]
pub fn aggregate_confidence(parsed: &ParsedRelease) -> Confidence {
    parsed.aggregate_confidence()
}

/// Whether `field` was extracted with at least `threshold` confidence.
#[must_use]
pub fn field_meets(parsed: &ParsedRelease, field: ParsedField, threshold: f32) -> bool {
    parsed.confidence_of(field).value() >= threshold
}

// Re-export the per-extractor entry points so each is individually testable and
// usable in isolation by callers that only want one slice of the parse.
pub use audio::extract as extract_audio;
pub use codec::extract as extract_codec;
pub use edition::extract as extract_edition;
pub use group::extract as extract_group;
pub use hdr::extract as extract_hdr;
pub use language::extract as extract_language;
pub use numbering::extract as extract_numbering;
pub use proper_repack::extract as extract_proper_repack;
pub use resolution::extract as extract_resolution;
pub use source::extract as extract_source;
pub use year::extract as extract_year;
