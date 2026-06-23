//! Identify-side coordinate normalization.
//!
//! Identify turns the *advertised* numbering the parser saw into the canonical
//! addressing the rest of the pipeline carries (see the `Coordinates` doc in
//! `cellarr-core` and `docs/02-data-model.md`). This module owns the anime
//! absolute-number remap: [`Coordinates::Absolute`] → [`Coordinates::Episode`]
//! `{ season, episode, absolute: Some(n) }`, driven by scene-mapping data
//! (TheXEM + anime-lists; see `docs/07-metadata-service.md`).
//!
//! The mapping data is fetched by `cellarr-meta` behind the seam; here it is
//! consumed through the small [`SceneMappingProvider`] trait so the remap logic
//! is testable against fixtures without a live metadata source. The actual
//! TheXEM/anime-lists adapter implements this trait in `cellarr-meta`.
//!
//! **Library-safety rule.** When the mapping does not cover an absolute number,
//! Identify returns [`MediaError::UnmappedAbsolute`] — it never guesses a
//! season/episode. The caller surfaces that for manual resolution rather than
//! force-fitting a wrong placement onto disk.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use cellarr_core::Coordinates;

use crate::error::MediaError;

/// One contiguous run of a series' absolute numbering and where it lands.
///
/// This is the distilled shape of a TheXEM / anime-lists mapping row: a season
/// whose episodes `1..=length` correspond to absolute numbers
/// `start_absolute ..= start_absolute + length - 1`. A series with a single
/// cour is one entry; a long-running anime split into TVDB "seasons" is several.
///
/// Modeling the mapping as ranges (rather than a per-episode table) keeps the
/// fixtures small and mirrors how the upstream data is published: each TVDB
/// season records the absolute number its first episode maps to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneRange {
    /// The TVDB season these episodes belong to.
    pub season: u32,
    /// The absolute episode number that season's episode 1 maps to.
    pub start_absolute: u32,
    /// How many episodes the season contains (so it covers
    /// `start_absolute ..= start_absolute + length - 1`).
    pub length: u32,
}

impl SceneRange {
    /// The inclusive last absolute number this range covers.
    #[must_use]
    fn end_absolute(&self) -> u32 {
        // length is the count of episodes; a 1-episode range covers exactly
        // start_absolute, hence the - 1.
        self.start_absolute + self.length.saturating_sub(1)
    }

    /// If `absolute` falls in this range, the season/episode it maps to.
    fn place(&self, absolute: u32) -> Option<(u32, u32)> {
        if self.length == 0 || absolute < self.start_absolute || absolute > self.end_absolute() {
            return None;
        }
        // Episode numbers are 1-based within the season.
        let episode = absolute - self.start_absolute + 1;
        Some((self.season, episode))
    }
}

/// A series' full absolute↔season/episode mapping, as several ordered ranges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneMapping {
    /// The series these ranges describe (for diagnostics / error messages).
    pub series: String,
    /// The ranges, in absolute order. Order is not relied on for correctness
    /// (lookup scans all), only for readable fixtures.
    pub ranges: Vec<SceneRange>,
}

impl SceneMapping {
    /// Place an absolute episode number onto a `(season, episode)`.
    ///
    /// Returns the **single** covering range's placement. If two ranges overlap
    /// the same absolute number the mapping is malformed (ambiguous) and the
    /// caller is told via [`MediaError::MalformedSceneMapping`] rather than an
    /// arbitrary pick being made.
    fn place(&self, absolute: u32) -> Result<Option<(u32, u32)>, MediaError> {
        let mut hit: Option<(u32, u32)> = None;
        for range in &self.ranges {
            if let Some(found) = range.place(absolute) {
                if hit.is_some() {
                    return Err(MediaError::MalformedSceneMapping {
                        series: self.series.clone(),
                        detail: format!("absolute {absolute} is covered by overlapping ranges"),
                    });
                }
                hit = Some(found);
            }
        }
        Ok(hit)
    }
}

/// The seam Identify reads scene mappings from.
///
/// `cellarr-meta` implements this over TheXEM + anime-lists (cached); tests mock
/// it. Keyed by the series' external id (e.g. the TVDB id) so a node's resolved
/// identity selects the right mapping.
#[async_trait]
pub trait SceneMappingProvider: Send + Sync {
    /// The typed error this provider reports.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Fetch the scene mapping for a series by external id, or `None` when the
    /// series has no mapping (a non-anime / non-scene series uses its native
    /// season/episode numbering and never reaches this path).
    async fn scene_mapping(
        &self,
        series_external_id: &str,
    ) -> Result<Option<SceneMapping>, Self::Error>;
}

/// Remap a single coordinate to its canonical form for the pipeline.
///
/// - [`Coordinates::Absolute`] is reconciled to [`Coordinates::Episode`] via the
///   series' scene mapping (this is the swampy anime case).
/// - Already-canonical coordinates ([`Coordinates::Episode`], `Movie`, `Track`,
///   `Book`) are returned unchanged.
/// - [`Coordinates::Daily`] and [`Coordinates::SeasonPack`] are *not* handled
///   here: Daily needs the series' air-date table and SeasonPack fans out to
///   many nodes — both are separate Identify concerns, so they are returned
///   unchanged and an out-of-band step handles them. (Documented so a caller is
///   never surprised that this function is absolute-only.)
///
/// # Errors
/// - [`MediaError::UnmappedAbsolute`] when no scene mapping (or no range within
///   it) covers the absolute number — surfaced for manual resolution, never
///   guessed.
/// - [`MediaError::MalformedSceneMapping`] when the mapping overlaps itself.
/// - the provider's own error, wrapped, when the lookup fails.
pub async fn remap_absolute<P: SceneMappingProvider>(
    provider: &P,
    series_external_id: &str,
    coords: &Coordinates,
) -> Result<Coordinates, IdentifyError<P::Error>> {
    let Coordinates::Absolute { number } = coords else {
        // Non-absolute coordinates are already canonical (or handled elsewhere).
        return Ok(coords.clone());
    };
    let number = *number;

    let mapping = provider
        .scene_mapping(series_external_id)
        .await
        .map_err(IdentifyError::Provider)?
        .ok_or_else(|| {
            IdentifyError::Media(MediaError::UnmappedAbsolute {
                series: series_external_id.to_string(),
                absolute: number,
            })
        })?;

    let placement = mapping.place(number).map_err(IdentifyError::Media)?;
    let (season, episode) = placement.ok_or_else(|| {
        IdentifyError::Media(MediaError::UnmappedAbsolute {
            series: mapping.series.clone(),
            absolute: number,
        })
    })?;

    Ok(Coordinates::Episode {
        season,
        episode,
        // Preserve the absolute number so downstream still knows the anime
        // numbering the release advertised.
        absolute: Some(number),
    })
}

/// Error from the absolute remap: either a logic failure ([`MediaError`]) or the
/// scene-mapping provider's own (I/O) error.
///
/// Two variants instead of swallowing the provider error into a string so the
/// caller can distinguish "this release needs manual resolution" (a `Media`
/// logic outcome) from "the metadata source was unreachable" (a `Provider`
/// transient).
#[derive(Debug, thiserror::Error)]
pub enum IdentifyError<E: std::error::Error + Send + Sync + 'static> {
    /// A logic failure (unmapped/malformed) — surface for manual resolution.
    #[error(transparent)]
    Media(#[from] MediaError),
    /// The scene-mapping provider failed (transient / I/O).
    #[error("scene-mapping provider failed: {0}")]
    Provider(#[source] E),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping() -> SceneMapping {
        SceneMapping {
            series: "Two Cour".to_string(),
            ranges: vec![
                SceneRange {
                    season: 1,
                    start_absolute: 1,
                    length: 12,
                },
                SceneRange {
                    season: 2,
                    start_absolute: 13,
                    length: 12,
                },
            ],
        }
    }

    #[test]
    fn range_places_within_and_rejects_outside() {
        let r = SceneRange {
            season: 2,
            start_absolute: 13,
            length: 12,
        };
        assert_eq!(r.place(13), Some((2, 1)));
        assert_eq!(r.place(24), Some((2, 12)));
        assert_eq!(r.place(12), None);
        assert_eq!(r.place(25), None);
    }

    #[test]
    fn zero_length_range_places_nothing() {
        let r = SceneRange {
            season: 1,
            start_absolute: 5,
            length: 0,
        };
        assert_eq!(r.place(5), None);
    }

    #[test]
    fn mapping_picks_the_single_covering_range() {
        let m = mapping();
        assert_eq!(m.place(13).unwrap(), Some((2, 1)));
        assert_eq!(m.place(12).unwrap(), Some((1, 12)));
        assert_eq!(m.place(99).unwrap(), None);
    }

    #[test]
    fn overlapping_ranges_are_reported_malformed() {
        let m = SceneMapping {
            series: "Bad".to_string(),
            ranges: vec![
                SceneRange {
                    season: 1,
                    start_absolute: 1,
                    length: 13,
                },
                SceneRange {
                    season: 2,
                    start_absolute: 13,
                    length: 12,
                },
            ],
        };
        assert!(matches!(
            m.place(13),
            Err(MediaError::MalformedSceneMapping { .. })
        ));
    }
}
