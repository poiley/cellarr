//! Scene mapping: remapping anime/scene numbering onto canonical episodes.
//!
//! Anime and scene releases are frequently numbered by **absolute episode**
//! (e.g. "Show - 28") while the canonical addressing the pipeline carries is
//! season/episode ([`cellarr_core::Coordinates::Episode`]). Reconciling the two
//! needs external mapping data (`docs/07-metadata-service.md`):
//!
//! - **TheXEM** (`thexem.info`) — scene ↔ TVDB episode-number mappings.
//! - **anime-lists** (`Anime-Lists/anime-lists`) — AniDB ↔ TheTVDB mappings,
//!   including the `defaulttvdbseason` and `episodeoffset` an absolute number
//!   needs to land in the right season.
//!
//! This module parses both documented shapes into one neutral [`SceneMap`] and
//! exposes [`SceneMap::remap_absolute`], which turns a transient
//! [`Coordinates::Absolute`] into a canonical [`Coordinates::Episode`] carrying
//! the original absolute number. The map is what an adapter's `scene_mapping`
//! returns (as JSON entries) and what Identify consumes.

use cellarr_core::Coordinates;
use serde::{Deserialize, Serialize};

use crate::error::MetaError;

/// One scene-mapping rule: a contiguous run of absolute episode numbers that map
/// into a single TVDB season at a fixed offset.
///
/// A typical anime-lists entry maps "AniDB absolute episodes 1.. → TVDB season
/// `tvdb_season`, starting at episode `episode_offset + 1`". TheXEM expresses the
/// same relationship per-episode; we normalize both into these runs so the remap
/// is a single offset add regardless of source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneRule {
    /// First absolute episode number this rule covers (inclusive).
    pub absolute_start: u32,
    /// Last absolute episode number this rule covers (inclusive). `None` means
    /// the rule extends to the end of the series.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_end: Option<u32>,
    /// The TVDB season these absolute numbers belong to.
    pub tvdb_season: u32,
    /// Value subtracted from the absolute number to get the in-season episode
    /// number. For a run starting at TVDB S2E1 with `absolute_start = 13`, the
    /// offset is 12 (13 → episode 1).
    pub episode_offset: u32,
}

impl SceneRule {
    /// Whether `absolute` falls within this rule's covered range.
    #[must_use]
    pub fn covers(&self, absolute: u32) -> bool {
        absolute >= self.absolute_start && self.absolute_end.is_none_or(|end| absolute <= end)
    }
}

/// A normalized scene map for one series: the union of its TheXEM + anime-lists
/// rules, sorted so the most specific (latest-starting) covering rule wins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneMap {
    /// The TVDB series id this map applies to, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<String>,
    /// The AniDB id this map applies to, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anidb_id: Option<String>,
    /// The mapping rules.
    pub rules: Vec<SceneRule>,
}

impl SceneMap {
    /// Build a map from contiguous season ranges, each described as "TVDB season
    /// `season` starts at absolute `start_absolute` and runs `length` episodes".
    ///
    /// This is the distilled shape the shared anime corpus (`corpus/anime/*`)
    /// uses; the episode offset is derived so the run's first absolute number
    /// lands on episode 1 (`offset = start_absolute - 1`).
    #[must_use]
    pub fn from_ranges(tvdb_id: Option<String>, ranges: &[(u32, u32, u32)]) -> Self {
        let rules = ranges
            .iter()
            .filter(|(_, _, length)| *length > 0)
            .map(|&(season, start_absolute, length)| SceneRule {
                absolute_start: start_absolute,
                absolute_end: Some(start_absolute + length - 1),
                tvdb_season: season,
                // First absolute of the run is episode 1, so subtract one less
                // than the start.
                episode_offset: start_absolute.saturating_sub(1),
            })
            .collect();
        Self {
            tvdb_id,
            anidb_id: None,
            rules,
        }
    }

    /// Remap a [`Coordinates::Absolute`] to a canonical
    /// [`Coordinates::Episode`], preserving the original absolute number.
    ///
    /// Other coordinate variants are returned unchanged: only `Absolute` is a
    /// scene-numbering artifact this map resolves, and a no-op for everything
    /// else keeps the call site branch-free.
    ///
    /// # Errors
    /// Returns [`MetaError::Unmappable`] when no rule covers the absolute number
    /// (the release is ahead of the published mapping) or when more than one rule
    /// covers it (a malformed/overlapping mapping). The library-safety rule is to
    /// surface these for manual resolution, never force-fit a guess — so an
    /// ambiguous number is an error, not an arbitrary pick.
    pub fn remap_absolute(&self, coords: &Coordinates) -> Result<Coordinates, MetaError> {
        let Coordinates::Absolute { number } = coords else {
            return Ok(coords.clone());
        };
        let number = *number;
        let covering: Vec<&SceneRule> = self.rules.iter().filter(|r| r.covers(number)).collect();
        match covering.as_slice() {
            [] => Err(MetaError::Unmappable {
                number,
                detail: "unmapped: no rule covers this absolute number".to_string(),
            }),
            [rule] => Ok(Coordinates::Episode {
                season: rule.tvdb_season,
                episode: number - rule.episode_offset,
                absolute: Some(number),
            }),
            _ => Err(MetaError::Unmappable {
                number,
                detail: "malformed: overlapping rules both claim this number".to_string(),
            }),
        }
    }
}

/// The anime-lists XML shape (`Anime-Lists/anime-lists`), reduced to the fields
/// the remap needs. The real file has many more attributes; we deserialize only
/// what drives numbering and ignore the rest.
///
/// Documented shape (synthetic fixture mirrors it):
/// ```xml
/// <anime anidbid="1234" tvdbid="81189" defaulttvdbseason="1" episodeoffset="0">
///   <mapping-list>
///     <mapping anidbseason="1" tvdbseason="2" start="1" end="12" offset="12"/>
///   </mapping-list>
/// </anime>
/// ```
#[derive(Debug, Deserialize)]
struct AnimeListEntry {
    #[serde(rename = "@anidbid")]
    anidbid: Option<String>,
    #[serde(rename = "@tvdbid")]
    tvdbid: Option<String>,
    #[serde(rename = "@defaulttvdbseason")]
    defaulttvdbseason: Option<String>,
    #[serde(rename = "@episodeoffset")]
    episodeoffset: Option<String>,
    #[serde(rename = "mapping-list")]
    mapping_list: Option<MappingList>,
}

#[derive(Debug, Deserialize)]
struct MappingList {
    #[serde(rename = "mapping", default)]
    mappings: Vec<AnimeMapping>,
}

#[derive(Debug, Deserialize)]
struct AnimeMapping {
    #[serde(rename = "@tvdbseason")]
    tvdbseason: Option<String>,
    #[serde(rename = "@start")]
    start: Option<String>,
    #[serde(rename = "@end")]
    end: Option<String>,
    #[serde(rename = "@offset")]
    offset: Option<String>,
}

fn parse_u32(s: &Option<String>) -> Option<u32> {
    s.as_ref().and_then(|v| v.trim().parse().ok())
}

/// Parse a single anime-lists `<anime>` XML element into a [`SceneMap`].
///
/// # Errors
/// Returns [`MetaError::Decode`] when the XML is malformed.
pub fn parse_anime_list_entry(xml: &str) -> Result<SceneMap, MetaError> {
    let entry: AnimeListEntry = quick_xml::de::from_str(xml).map_err(|e| MetaError::Decode {
        src: "anime-lists",
        detail: e.to_string(),
    })?;

    let mut rules = Vec::new();

    // The explicit per-season mappings.
    if let Some(list) = &entry.mapping_list {
        for m in &list.mappings {
            if let Some(season) = parse_u32(&m.tvdbseason) {
                rules.push(SceneRule {
                    absolute_start: parse_u32(&m.start).unwrap_or(1),
                    absolute_end: parse_u32(&m.end),
                    tvdb_season: season,
                    episode_offset: parse_u32(&m.offset).unwrap_or(0),
                });
            }
        }
    }

    // The default-season fallback covers the absolute numbers *before* the first
    // explicit mapping (the common single-season-anime case, and the episodes
    // ahead of a later cour). It is deliberately bounded so it never overlaps an
    // explicit rule — overlap would make a covered number ambiguous, which the
    // remap (correctly) treats as a malformed mapping.
    if let Some(default_season) = parse_u32(&entry.defaulttvdbseason) {
        let first_explicit = rules.iter().map(|r| r.absolute_start).min();
        let absolute_end = first_explicit.map(|start| start.saturating_sub(1));
        // Skip the fallback entirely if an explicit mapping already starts at 1.
        if absolute_end != Some(0) {
            rules.push(SceneRule {
                absolute_start: 1,
                absolute_end,
                tvdb_season: default_season,
                episode_offset: parse_u32(&entry.episodeoffset).unwrap_or(0),
            });
        }
    }

    Ok(SceneMap {
        tvdb_id: entry.tvdbid,
        anidb_id: entry.anidbid,
        rules,
    })
}

/// The TheXEM JSON shape (`thexem.info` `map/all` response), reduced to the
/// fields the remap needs.
///
/// Documented shape (synthetic fixture mirrors it): an array of per-episode
/// entries each pairing a `scene` absolute number with a `tvdb` season/episode.
/// We collapse consecutive entries that share a season + constant offset into
/// [`SceneRule`] runs.
#[derive(Debug, Deserialize)]
struct XemResponse {
    #[serde(default)]
    data: Vec<XemEntry>,
}

#[derive(Debug, Deserialize)]
struct XemEntry {
    scene: XemNumber,
    tvdb: XemNumber,
}

#[derive(Debug, Deserialize)]
struct XemNumber {
    season: u32,
    #[allow(dead_code)]
    episode: u32,
    #[serde(default)]
    absolute: u32,
}

/// Parse a TheXEM `map/all` JSON body into a [`SceneMap`].
///
/// # Errors
/// Returns [`MetaError::Decode`] when the JSON does not match the documented
/// shape.
pub fn parse_xem(json: &[u8], tvdb_id: Option<String>) -> Result<SceneMap, MetaError> {
    let resp: XemResponse = serde_json::from_slice(json).map_err(|e| MetaError::Decode {
        src: "thexem",
        detail: e.to_string(),
    })?;

    // Collapse per-episode rows into contiguous (season, offset) runs.
    let mut rules: Vec<SceneRule> = Vec::new();
    for entry in &resp.data {
        if entry.scene.absolute == 0 {
            continue;
        }
        let offset = entry.scene.absolute.saturating_sub(entry.tvdb.episode);
        match rules.last_mut() {
            Some(last)
                if last.tvdb_season == entry.tvdb.season
                    && last.episode_offset == offset
                    && last.absolute_end == Some(entry.scene.absolute - 1) =>
            {
                last.absolute_end = Some(entry.scene.absolute);
            }
            _ => rules.push(SceneRule {
                absolute_start: entry.scene.absolute,
                absolute_end: Some(entry.scene.absolute),
                tvdb_season: entry.tvdb.season,
                episode_offset: offset,
            }),
        }
    }

    Ok(SceneMap {
        tvdb_id,
        anidb_id: None,
        rules,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_season_map() -> SceneMap {
        // S1 covers absolute 1..12 (offset 0); S2 covers 13..24 (offset 12).
        SceneMap {
            tvdb_id: Some("246521".to_string()),
            anidb_id: Some("9876".to_string()),
            rules: vec![
                SceneRule {
                    absolute_start: 1,
                    absolute_end: Some(12),
                    tvdb_season: 1,
                    episode_offset: 0,
                },
                SceneRule {
                    absolute_start: 13,
                    absolute_end: Some(24),
                    tvdb_season: 2,
                    episode_offset: 12,
                },
            ],
        }
    }

    #[test]
    fn remaps_first_season_absolute_to_episode() {
        let map = two_season_map();
        let out = map
            .remap_absolute(&Coordinates::Absolute { number: 5 })
            .unwrap();
        assert_eq!(
            out,
            Coordinates::Episode {
                season: 1,
                episode: 5,
                absolute: Some(5)
            }
        );
    }

    #[test]
    fn remaps_second_season_absolute_across_offset() {
        let map = two_season_map();
        // Absolute 13 is the first episode of TVDB season 2.
        let out = map
            .remap_absolute(&Coordinates::Absolute { number: 13 })
            .unwrap();
        assert_eq!(
            out,
            Coordinates::Episode {
                season: 2,
                episode: 1,
                absolute: Some(13)
            }
        );
    }

    #[test]
    fn remap_is_noop_for_non_absolute_coordinates() {
        let map = two_season_map();
        let already = Coordinates::Episode {
            season: 3,
            episode: 4,
            absolute: None,
        };
        assert_eq!(map.remap_absolute(&already).unwrap(), already);
    }

    #[test]
    fn remap_errors_when_no_rule_covers() {
        let map = two_season_map();
        let err = map
            .remap_absolute(&Coordinates::Absolute { number: 999 })
            .unwrap_err();
        assert!(matches!(err, MetaError::Unmappable { number: 999, .. }));
    }

    #[test]
    fn remap_reports_malformed_on_overlapping_rules() {
        // Two rules both cover absolute 13 — a mapping data bug. The remap must
        // surface it, never pick one arbitrarily.
        let map = SceneMap {
            tvdb_id: None,
            anidb_id: None,
            rules: vec![
                SceneRule {
                    absolute_start: 1,
                    absolute_end: Some(13),
                    tvdb_season: 1,
                    episode_offset: 0,
                },
                SceneRule {
                    absolute_start: 13,
                    absolute_end: Some(24),
                    tvdb_season: 2,
                    episode_offset: 12,
                },
            ],
        };
        let err = map
            .remap_absolute(&Coordinates::Absolute { number: 13 })
            .unwrap_err();
        match err {
            MetaError::Unmappable { number, detail } => {
                assert_eq!(number, 13);
                assert!(detail.contains("malformed"));
            }
            other => panic!("expected Unmappable, got {other:?}"),
        }
    }

    #[test]
    fn parses_anime_list_default_and_mapping() {
        let xml = r#"<anime anidbid="9876" tvdbid="246521" defaulttvdbseason="1" episodeoffset="0">
            <mapping-list>
                <mapping anidbseason="1" tvdbseason="2" start="13" end="24" offset="12"/>
            </mapping-list>
        </anime>"#;
        let map = parse_anime_list_entry(xml).unwrap();
        assert_eq!(map.tvdb_id.as_deref(), Some("246521"));
        // The default-season rule plus the explicit S2 mapping.
        assert!(map
            .rules
            .iter()
            .any(|r| r.tvdb_season == 2 && r.episode_offset == 12));
        // Absolute 20 lands in S2 via the explicit mapping (not the default S1).
        let out = map
            .remap_absolute(&Coordinates::Absolute { number: 20 })
            .unwrap();
        assert_eq!(
            out,
            Coordinates::Episode {
                season: 2,
                episode: 8,
                absolute: Some(20)
            }
        );
    }

    #[test]
    fn parses_xem_into_rules() {
        let json = br#"{"data":[
            {"scene":{"season":1,"episode":1,"absolute":1},"tvdb":{"season":1,"episode":1,"absolute":1}},
            {"scene":{"season":1,"episode":2,"absolute":2},"tvdb":{"season":1,"episode":2,"absolute":2}},
            {"scene":{"season":2,"episode":1,"absolute":3},"tvdb":{"season":2,"episode":1,"absolute":3}}
        ]}"#;
        let map = parse_xem(json, Some("123".to_string())).unwrap();
        // Two contiguous S1 rows collapse to one run; the S2 row starts a new run.
        assert_eq!(map.rules.len(), 2);
        let s2 = map
            .remap_absolute(&Coordinates::Absolute { number: 3 })
            .unwrap();
        assert_eq!(
            s2,
            Coordinates::Episode {
                season: 2,
                episode: 1,
                absolute: Some(3)
            }
        );
    }
}
