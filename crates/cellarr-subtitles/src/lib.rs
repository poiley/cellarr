//! Native subtitle search & download — cellarr's built-in "Bazarr".
//!
//! A [`SubtitleProvider`] turns a [`SubtitleQuery`] (external ids + wanted
//! languages) into ranked [`SubtitleMatch`]es and downloads the chosen one's
//! bytes. Provider-specific detail (OpenSubtitles' JSON, auth, scoring) lives
//! behind the trait; everything talks HTTP through the [`Fetcher`] seam so the
//! record/replay tests exercise the full path with no live provider — the same
//! discipline `cellarr-meta` uses for TMDb/TheTVDB.
//!
//! The pipeline layer (a `SubtitleSearch` job) drives a provider: for each media
//! file missing a wanted language it searches, picks the best match, downloads,
//! writes the sidecar beside the media, and records a `subtitle` row.

mod error;
mod http;
mod opensubtitles;
mod provider;

pub use error::SubtitleError;
pub use http::{Fetcher, HttpResponse, RecordedFetcher, ReqwestFetcher};
pub use opensubtitles::{OpenSubtitles, OpenSubtitlesConfig};
pub use provider::{SubtitleMatch, SubtitleProvider, SubtitleQuery};
