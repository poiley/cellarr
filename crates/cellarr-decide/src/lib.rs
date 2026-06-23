//! cellarr-decide — the decision engine.
//!
//! Pure, deterministic scoring and grab/upgrade/reject decisions over
//! `cellarr-core` types, with TRaSH-compatible custom-format semantics. No I/O.
//!
//! # What it does
//!
//! - [`score`] / [`score_detailed`] — a release's custom-format score is the
//!   **sum** of the scores of every matching custom format.
//! - [`MatchContext`] — full custom-format matching over a [`Release`] plus its
//!   parse, including the title-regex, indexer-flag, and size conditions that
//!   `cellarr-core` cannot evaluate on its own.
//! - [`decide`] — the verdict (grab / upgrade / reject) with a structured reason,
//!   enforcing the precedence contract in `docs/05-decision-engine.md`.
//! - [`import_trash_custom_formats`] — load community TRaSH/Recyclarr custom
//!   formats so users keep their existing tuning.
//!
//! # The precedence contract
//!
//! 1. Hard rejects first (disallowed quality, below min CF score, blocklist,
//!    size, language).
//! 2. **Quality rank dominates** — never downgrade quality to chase CF score.
//! 3. CF score breaks ties within equal quality.
//! 4. Upgrade only on a real upgrade, with upgrades allowed and **both** cutoffs
//!    (quality and CF score) unmet; stop once both are met (no churn).
//! 5. Proper/repack per policy.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod decide;
pub mod error;
pub mod matching;
pub mod quality;
pub mod scoring;
pub mod trash;

pub use decide::{decide, DecisionContext, OnDiskFile, ProperRepackPolicy};
pub use error::DecideError;
pub use matching::MatchContext;
pub use quality::{on_disk_from_media_file, resolve_candidate_quality};
pub use scoring::{score, score_detailed, MatchedFormat, ScoreBreakdown};
pub use trash::{
    convert, import_trash_custom_formats, import_trash_custom_formats_counted,
    import_trash_custom_formats_counted_for_app, import_trash_custom_formats_for_app, TrashApp,
    TrashCustomFormat, TrashFields, TrashImportReport, TrashSpecification,
};
