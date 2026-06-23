//! Typed errors for the metadata service.
//!
//! `cellarr-meta` is an I/O-bearing crate, so unlike core it must describe
//! transport and decoding failures as well as the graceful-degradation cases the
//! daemon depends on (no key configured, source offline). Each [`MetadataSource`]
//! impl reports this type as its associated `Error`.
//!
//! [`MetadataSource`]: cellarr_core::MetadataSource

/// Errors produced by a metadata source or the cache/mapping layers around it.
///
/// `Clone` is derived so the cache can hand each coalesced caller its own owned
/// error when a shared (`Arc`-wrapped) load fails — see
/// [`MetaCache::get_or_try_insert_with`](crate::cache::MetaCache::get_or_try_insert_with).
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum MetaError {
    /// No API key/credential is configured for a source that requires one
    /// (TMDb, TheTVDB). This is *not* a hard error to the daemon: callers treat
    /// it as "this source is unavailable" and degrade gracefully (offline is a
    /// non-negotiable), but it is surfaced rather than silently swallowed.
    #[error("metadata source '{src}' has no API key configured")]
    NoCredential {
        /// The source that needs a credential (e.g. `tmdb`, `thetvdb`).
        src: &'static str,
    },

    /// The source was reachable but answered with a non-success status. The
    /// status is kept so callers can distinguish auth (401/403), rate-limit
    /// (429), and not-found (404) without re-parsing a message string.
    #[error("metadata source '{src}' returned HTTP {status}")]
    Http {
        /// The source that answered.
        src: &'static str,
        /// The HTTP status code returned.
        status: u16,
    },

    /// The transport itself failed (DNS, TLS, connection, timeout). On these the
    /// daemon degrades to cached/offline behavior.
    #[error("transport error talking to '{src}': {detail}")]
    Transport {
        /// The source we were talking to.
        src: &'static str,
        /// Human-readable transport detail.
        detail: String,
    },

    /// A response body did not match the shape we normalize from. Records the
    /// source and a short reason; the raw body is intentionally not embedded.
    #[error("could not decode '{src}' response: {detail}")]
    Decode {
        /// The source whose payload failed to decode.
        src: &'static str,
        /// What specifically went wrong.
        detail: String,
    },

    /// A scene mapping could not place an absolute episode number: either no rule
    /// covers it (the release is ahead of the published mapping) or the mapping
    /// is malformed (overlapping rules both claim it). The library-safety rule is
    /// to **surface** this for manual resolution, never force-fit a guess.
    #[error("scene mapping cannot place absolute episode {number}: {detail}")]
    Unmappable {
        /// The absolute number that could not be placed.
        number: u32,
        /// Why it could not be placed (`unmapped` vs `malformed`).
        detail: String,
    },
}

/// Convenience alias for fallible metadata operations.
pub type Result<T> = std::result::Result<T, MetaError>;
