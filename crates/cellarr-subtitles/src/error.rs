//! The subtitle-provider error type.
//!
//! Mirrors [`cellarr_meta`]'s `MetaError` shape: every variant carries the
//! provider name so a caller knows which source failed, and the set distinguishes
//! "no credential" / transport / HTTP-status / decode / not-found so the search
//! job can degrade gracefully (a missing key is not an error worth paging on).

/// An error from a subtitle provider.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum SubtitleError {
    /// The provider needs a credential (API key / login) that is not configured.
    #[error("subtitle provider '{src}' has no credential configured")]
    NoCredential {
        /// The provider name.
        src: &'static str,
    },

    /// The provider returned a non-success HTTP status.
    #[error("subtitle provider '{src}' returned HTTP {status}")]
    Http {
        /// The provider name.
        src: &'static str,
        /// The HTTP status code.
        status: u16,
    },

    /// A transport-level failure talking to the provider.
    #[error("transport error talking to '{src}': {detail}")]
    Transport {
        /// The provider name.
        src: &'static str,
        /// A short description of the failure.
        detail: String,
    },

    /// The provider's response could not be decoded into the expected shape.
    #[error("could not decode '{src}' response: {detail}")]
    Decode {
        /// The provider name.
        src: &'static str,
        /// A short description of the decode failure.
        detail: String,
    },
}
