//! Indexer candidates and their identification results.
//!
//! A [`Release`] is a raw candidate as advertised by an indexer at Discover
//! time. After parsing and identification it becomes associated with one or more
//! [`ContentMatch`] values that say which content node(s) it satisfies and how
//! confidently.

use serde::{Deserialize, Serialize};

use crate::ids::IndexerId;
use crate::media::{ContentRef, Coordinates};
use crate::parsed::{Confidence, ParsedRelease};

/// The kind of thing a release/file is, derived once from its parsed numbering.
///
/// This is **persisted durable state**, set at grab/import time from the parse,
/// so the upgrade/reconcile decision can read it back instead of re-parsing the
/// advertised title every cycle. Re-parsing each cycle is what causes the
/// originals' infamous season-pack re-grab loops: a previously-grabbed full
/// season is re-parsed, looks "new", and is grabbed again. By writing the type
/// down on the grab and on the resulting `media_file` (and `history`), the
/// reconcile path knows "I already hold this full season" without re-deriving it.
///
/// Serializes in `snake_case` so it reads naturally in the stored TEXT column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseType {
    /// A movie release (no episodic numbering).
    Movie,
    /// A single TV episode.
    SingleEpisode,
    /// A release spanning several discrete episodes (a multi-episode file).
    MultiEpisode,
    /// A whole-season release (a season pack). The flag the re-grab-loop fix
    /// keys on.
    FullSeason,
    /// A date-addressed broadcast (a daily show).
    Daily,
    /// An anime absolute-numbered release that Identify has not yet remapped.
    Absolute,
    /// A music track / album release.
    Track,
    /// A book release.
    Book,
    /// Numbering that does not fit the above (or an empty parse).
    Other,
}

impl ReleaseType {
    /// Derive the release type from a parse's coordinates.
    ///
    /// A release that carries several distinct episode coordinates is a
    /// [`ReleaseType::MultiEpisode`]; a season-pack coordinate is a
    /// [`ReleaseType::FullSeason`]. The derivation is total and deterministic so
    /// the same parse always yields the same persisted type.
    #[must_use]
    pub fn from_coordinates(coords: &[Coordinates]) -> Self {
        // A season pack present anywhere makes the whole release a full season —
        // it is the dominant, loop-prone type and must win regardless of order.
        if coords
            .iter()
            .any(|c| matches!(c, Coordinates::SeasonPack { .. }))
        {
            return ReleaseType::FullSeason;
        }
        let episodes = coords
            .iter()
            .filter(|c| matches!(c, Coordinates::Episode { .. }))
            .count();
        if episodes > 1 {
            return ReleaseType::MultiEpisode;
        }
        match coords.first() {
            Some(Coordinates::Movie) => ReleaseType::Movie,
            Some(Coordinates::Episode { .. }) => ReleaseType::SingleEpisode,
            Some(Coordinates::Daily { .. }) => ReleaseType::Daily,
            Some(Coordinates::Absolute { .. }) => ReleaseType::Absolute,
            Some(Coordinates::Track { .. }) => ReleaseType::Track,
            Some(Coordinates::Book { .. }) => ReleaseType::Book,
            // SeasonPack handled above; an empty parse has no type.
            Some(Coordinates::SeasonPack { .. }) | None => ReleaseType::Other,
        }
    }

    /// Derive the release type from a [`ParsedRelease`] (its coordinates).
    #[must_use]
    pub fn from_parsed(parsed: &ParsedRelease) -> Self {
        Self::from_coordinates(&parsed.coordinates)
    }

    /// Whether this is a whole-season release — the predicate the reconcile path
    /// uses to recognize an already-held season pack and refuse a re-grab loop.
    #[must_use]
    pub fn is_full_season(self) -> bool {
        matches!(self, ReleaseType::FullSeason)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::Coordinates;

    #[test]
    fn season_pack_dominates_and_yields_full_season() {
        let rt = ReleaseType::from_coordinates(&[
            Coordinates::Episode {
                season: 2,
                episode: 1,
                absolute: None,
            },
            Coordinates::SeasonPack { season: 2 },
        ]);
        assert_eq!(rt, ReleaseType::FullSeason);
        assert!(rt.is_full_season());
    }

    #[test]
    fn multiple_episodes_are_multi_episode() {
        let rt = ReleaseType::from_coordinates(&[
            Coordinates::Episode {
                season: 1,
                episode: 1,
                absolute: None,
            },
            Coordinates::Episode {
                season: 1,
                episode: 2,
                absolute: None,
            },
        ]);
        assert_eq!(rt, ReleaseType::MultiEpisode);
        assert!(!rt.is_full_season());
    }

    #[test]
    fn single_variants_map_one_to_one() {
        assert_eq!(
            ReleaseType::from_coordinates(&[Coordinates::Movie]),
            ReleaseType::Movie
        );
        assert_eq!(
            ReleaseType::from_coordinates(&[Coordinates::Episode {
                season: 1,
                episode: 1,
                absolute: None
            }]),
            ReleaseType::SingleEpisode
        );
        assert_eq!(
            ReleaseType::from_coordinates(&[Coordinates::Absolute { number: 1071 }]),
            ReleaseType::Absolute
        );
        assert_eq!(
            ReleaseType::from_coordinates(&[Coordinates::Daily {
                date: "2024-01-01".into()
            }]),
            ReleaseType::Daily
        );
    }

    #[test]
    fn an_empty_parse_has_no_type() {
        assert_eq!(ReleaseType::from_coordinates(&[]), ReleaseType::Other);
    }
}
