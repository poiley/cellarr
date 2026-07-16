//! The provider-agnostic subtitle search/download seam.
//!
//! A [`SubtitleProvider`] turns "what I have and what languages I want" into
//! ranked [`SubtitleMatch`]es, then downloads the chosen one's bytes. Everything
//! source-specific (OpenSubtitles' JSON, its auth, its scoring) lives behind this
//! trait so the search job stays provider-agnostic — exactly how `cellarr-meta`
//! hides TMDb/TheTVDB behind one normalized schema.

use async_trait::async_trait;
use cellarr_core::MediaType;

use crate::error::SubtitleError;

/// What is known about the item we want subtitles for. Providers prefer an
/// external id (imdb is the most widely supported) and fall back to a title
/// query; TV adds season/episode so a provider can pick the right episode.
#[derive(Debug, Clone, Default)]
pub struct SubtitleQuery {
    /// Movie or Tv (episode).
    pub media_type: Option<MediaType>,
    /// IMDb id without the `tt` prefix stripped (e.g. `tt0133093`) — the most
    /// widely supported key.
    pub imdb_id: Option<String>,
    /// TMDb numeric id, when imdb is absent.
    pub tmdb_id: Option<String>,
    /// Season number (TV only).
    pub season: Option<u32>,
    /// Episode number (TV only).
    pub episode: Option<u32>,
    /// A free-text title fallback when no external id is available.
    pub query: Option<String>,
    /// The release/file name on disk, used to prefer a matching release.
    pub release_name: Option<String>,
    /// The wanted languages, as ISO-639-1 codes (`en`, `es`). A provider searches
    /// all of them and returns matches tagged with which it found.
    pub languages: Vec<String>,
}

/// One candidate subtitle a provider found. `id` is opaque to the caller — it is
/// whatever the provider needs to [`download`](SubtitleProvider::download) it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleMatch {
    /// The provider that produced this match (`opensubtitles`).
    pub provider: &'static str,
    /// The provider's own id for the downloadable file.
    pub id: String,
    /// ISO-639-1 language code.
    pub language: String,
    /// The release/file name the subtitle was authored against, when known — used
    /// to prefer an exact release match over a generic one.
    pub release_name: Option<String>,
    /// A forced-narrative-only subtitle (foreign-dialogue lines).
    pub forced: bool,
    /// A hearing-impaired (SDH) subtitle.
    pub hearing_impaired: bool,
    /// A provider-normalized ranking score (higher = better): popularity, rating,
    /// and trust folded into one comparable integer.
    pub score: i32,
    /// The subtitle file format/extension (`srt`, `ass`, …).
    pub format: String,
}

/// A subtitle source: search for matches, then download one's bytes.
#[async_trait]
pub trait SubtitleProvider: Send + Sync {
    /// The provider's stable name (`opensubtitles`).
    fn name(&self) -> &'static str;

    /// Find candidate subtitles for `query`, ranked best-first per language.
    async fn search(&self, query: &SubtitleQuery) -> Result<Vec<SubtitleMatch>, SubtitleError>;

    /// Download the chosen match, returning the raw subtitle file bytes.
    async fn download(&self, m: &SubtitleMatch) -> Result<Vec<u8>, SubtitleError>;
}
