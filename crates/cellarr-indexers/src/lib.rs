//! cellarr-indexers — indexer integrations.
//!
//! [`cellarr_core::Indexer`] implementations for the **Torznab** and **Newznab**
//! protocols, plus a skeleton **Cardigann** YAML engine that interprets community
//! tracker definitions at runtime. Every adapter normalizes results into
//! [`cellarr_core::Release`] so downstream stages stay indexer-agnostic.
//!
//! Design follows `docs/specs/cellarr-indexers.md` and `docs/06-integrations.md`:
//!
//! - **Capabilities first.** [`TorznabIndexer`]/[`NewznabIndexer`] fetch and cache
//!   `t=caps` before searching and only send modes/params the server advertises —
//!   categories are read from caps, never hardcoded.
//! - **Per-host rate limiting** via [`HostRateLimiter`] (governor), keyed on host
//!   so indexers sharing a tracker host share the budget the tracker enforces.
//! - **Cardigann definitions are data, not code,** and are never vendored here for
//!   licensing reasons; the engine interprets a user-supplied definition.
//! - **Record/replay only.** The HTTP seam is the [`Fetcher`] trait so tests feed
//!   recorded responses; live indexers are never a test dependency.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod caps;
pub mod cardigann;
pub mod error;
pub mod feed;
pub mod http;
pub mod ratelimit;
pub mod torznab;

pub use caps::{parse_caps, Caps, Category, SearchMode};
pub use cardigann::{CardigannIndexer, Definition};
pub use error::{IndexerError, Result};
pub use feed::parse_feed;
pub use http::{Fetcher, ReqwestFetcher};
pub use ratelimit::HostRateLimiter;
pub use torznab::{NabIndexer, NewznabIndexer, TorznabIndexer};
