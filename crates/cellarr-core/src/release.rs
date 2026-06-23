//! Indexer candidates and their identification results.
//!
//! A [`Release`] is a raw candidate as advertised by an indexer at Discover
//! time. After parsing and identification it becomes associated with one or more
//! [`ContentMatch`] values that say which content node(s) it satisfies and how
//! confidently.

use serde::{Deserialize, Serialize};

use crate::ids::IndexerId;
use crate::media::ContentRef;
use crate::parsed::{Confidence, ParsedRelease};

/// The download protocol a release uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// BitTorrent.
    Torrent,
    /// Usenet.
    Usenet,
}

/// A candidate release as advertised by an indexer.
///
/// The `title` is advertising and may lie — it is parsed at Discover time to
/// decide whether to grab, and the actual files are re-parsed at Import time
/// before anything touches the library (see `docs/03-pipeline.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    /// The indexer that returned this candidate.
    pub indexer_id: IndexerId,
    /// The advertised release title.
    pub title: String,
    /// The download URL or magnet link.
    pub download_url: String,
    /// Optional info/GUID URL that uniquely identifies the release on the indexer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guid: Option<String>,
    /// Download protocol.
    pub protocol: Protocol,
    /// Size in bytes, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Seeders, for torrents, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seeders: Option<u32>,
    /// Indexer flags (e.g. "freeleech"), normalized to lowercase strings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexer_flags: Vec<String>,
}

/// A release together with the parse used to reason about it.
///
/// Identify operates on this pairing; the parse is kept alongside the raw
/// release so the decision log can record exactly what was believed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedCandidate {
    /// The raw candidate.
    pub release: Release,
    /// The structured parse of `release.title`.
    pub parsed: ParsedRelease,
}

/// The result of identifying a parsed candidate against the library: which
/// content node it satisfies and with what confidence.
///
/// A single multi-episode release produces several `ContentMatch` values — one
/// per episode node it covers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentMatch {
    /// The content node this candidate satisfies.
    pub content_ref: ContentRef,
    /// How confident the identifier is in this match.
    pub confidence: Confidence,
}
