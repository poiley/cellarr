//! Error types for the decision engine.

use thiserror::Error;

/// Errors the decision engine can raise.
///
/// Decision and scoring are pure and total over well-formed inputs, so the only
/// fallible surface is compiling user-supplied regexes (from custom-format
/// title conditions) and importing TRaSH-format JSON.
#[derive(Debug, Error)]
pub enum DecideError {
    /// A custom-format title condition carried a regex that failed to compile.
    #[error("invalid release-title regex in custom format {format:?}: {source}")]
    InvalidRegex {
        /// The name of the custom format whose condition failed to compile.
        format: String,
        /// The underlying regex compilation error.
        #[source]
        source: regex::Error,
    },

    /// TRaSH-format custom-format JSON could not be parsed.
    #[error("could not parse TRaSH custom-format JSON: {0}")]
    TrashJson(#[from] serde_json::Error),

    /// A TRaSH custom format used a field implementation we do not model.
    #[error("unsupported TRaSH specification {implementation:?} in custom format {format:?}")]
    UnsupportedTrashSpec {
        /// The custom format that contained the unsupported specification.
        format: String,
        /// The TRaSH `implementation` discriminator we did not recognize.
        implementation: String,
    },
}
