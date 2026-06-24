//! Sonarr-style monitoring options: which episodes a newly-added series should be
//! monitored for.
//!
//! When a series is added, the user picks *which* of its episodes cellarr should
//! actively try to acquire — the [`MonitorOption`] in the v3 add `addOptions.monitor`
//! field. The choice is purely a function of each episode's `(season, episode)`
//! coordinates plus whether the episode has already aired, so it lives in core
//! (pure, exhaustively testable) and is applied by whoever writes the episode
//! nodes (the add path / the manual-import commit). It is deliberately free of any
//! database or clock dependency: the caller supplies the air state.
//!
//! These mirror the options Sonarr exposes on its add screen. They are learned
//! clean-room from the *behavior* (which episodes end up monitored), not
//! transcribed.

use serde::{Deserialize, Serialize};

/// One episode's identity for the monitor computation: its season/episode number
/// and whether it has already aired (so `Future`/`Existing`/`Missing` can split on
/// it). The caller resolves "has aired" however it likes (a stored air date
/// compared against now); the computation itself never reads a clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodeFacts {
    /// Season number (specials are conventionally season 0).
    pub season: u32,
    /// Episode number within the season.
    pub episode: u32,
    /// Whether the episode has already aired (its air date is in the past).
    pub aired: bool,
    /// Whether a file for this episode is already present on disk.
    pub has_file: bool,
}

/// The Sonarr-style monitor selection applied to a series' episodes on add.
///
/// Serializes in the camelCase spellings the v3 `addOptions.monitor` field uses,
/// so the shim can deserialize the wire value straight into this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum MonitorOption {
    /// Monitor every episode (the default — "Monitor all episodes").
    #[default]
    All,
    /// Monitor only episodes that have already aired.
    Existing,
    /// Monitor only episodes that have **not** yet aired.
    Future,
    /// Monitor only episodes that have aired but are not yet on disk (the
    /// "missing" set — what an initial backfill targets).
    Missing,
    /// Monitor only the first season.
    FirstSeason,
    /// Monitor only the most recent (highest-numbered) regular season.
    LastSeason,
    /// Monitor only the series pilot (season 1, episode 1).
    Pilot,
    /// Monitor nothing (add the series unmonitored).
    None,
}

impl MonitorOption {
    /// The highest **regular** (non-special) season number among `episodes`, or
    /// `None` when there are no regular-season episodes. Season 0 (specials) is
    /// excluded so `LastSeason` never resolves to the specials bucket.
    fn last_regular_season(episodes: &[EpisodeFacts]) -> Option<u32> {
        episodes
            .iter()
            .filter(|e| e.season > 0)
            .map(|e| e.season)
            .max()
    }

    /// The lowest **regular** (non-special) season number among `episodes`, or
    /// `None`. Used by `FirstSeason` so a series whose only specials are season 0
    /// still targets its first real season.
    fn first_regular_season(episodes: &[EpisodeFacts]) -> Option<u32> {
        episodes
            .iter()
            .filter(|e| e.season > 0)
            .map(|e| e.season)
            .min()
    }

    /// Whether a single episode should be monitored under this option, given the
    /// season bounds computed once over the whole set.
    fn includes(
        self,
        ep: &EpisodeFacts,
        first_season: Option<u32>,
        last_season: Option<u32>,
    ) -> bool {
        match self {
            MonitorOption::All => true,
            MonitorOption::None => false,
            MonitorOption::Existing => ep.aired,
            MonitorOption::Future => !ep.aired,
            MonitorOption::Missing => ep.aired && !ep.has_file,
            MonitorOption::FirstSeason => Some(ep.season) == first_season,
            MonitorOption::LastSeason => Some(ep.season) == last_season,
            MonitorOption::Pilot => ep.season == 1 && ep.episode == 1,
        }
    }

    /// Compute the monitored flag for every episode under this option.
    ///
    /// Returns a `Vec<bool>` aligned 1:1 with `episodes` (index `i` is the
    /// monitored decision for `episodes[i]`), so the caller can zip it back onto
    /// the episode nodes it is about to write. The season-relative options
    /// (`FirstSeason`/`LastSeason`) resolve their target season once over the
    /// whole set, so the result is consistent regardless of input order.
    #[must_use]
    pub fn monitored_flags(self, episodes: &[EpisodeFacts]) -> Vec<bool> {
        let first = Self::first_regular_season(episodes);
        let last = Self::last_regular_season(episodes);
        episodes
            .iter()
            .map(|ep| self.includes(ep, first, last))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small fixture series: S00 (a special), S01E01..E02, S02E01..E02. The
    /// `aired`/`has_file` flags are chosen so each split option has a distinct,
    /// asserting result.
    fn fixture() -> Vec<EpisodeFacts> {
        vec![
            // A special (season 0): aired, no file.
            EpisodeFacts {
                season: 0,
                episode: 1,
                aired: true,
                has_file: false,
            },
            // S01E01 — aired, on disk (the pilot).
            EpisodeFacts {
                season: 1,
                episode: 1,
                aired: true,
                has_file: true,
            },
            // S01E02 — aired, missing.
            EpisodeFacts {
                season: 1,
                episode: 2,
                aired: true,
                has_file: false,
            },
            // S02E01 — aired, missing.
            EpisodeFacts {
                season: 2,
                episode: 1,
                aired: true,
                has_file: false,
            },
            // S02E02 — not yet aired.
            EpisodeFacts {
                season: 2,
                episode: 2,
                aired: false,
                has_file: false,
            },
        ]
    }

    #[test]
    fn all_monitors_every_episode_including_specials() {
        let eps = fixture();
        assert_eq!(
            MonitorOption::All.monitored_flags(&eps),
            vec![true, true, true, true, true]
        );
    }

    #[test]
    fn none_monitors_nothing() {
        let eps = fixture();
        assert_eq!(
            MonitorOption::None.monitored_flags(&eps),
            vec![false, false, false, false, false]
        );
    }

    #[test]
    fn existing_monitors_only_aired_episodes() {
        let eps = fixture();
        // Every aired episode (S00E01, S01E01, S01E02, S02E01) is monitored; the
        // unaired S02E02 is not.
        assert_eq!(
            MonitorOption::Existing.monitored_flags(&eps),
            vec![true, true, true, true, false]
        );
    }

    #[test]
    fn future_monitors_only_unaired_episodes() {
        let eps = fixture();
        assert_eq!(
            MonitorOption::Future.monitored_flags(&eps),
            vec![false, false, false, false, true]
        );
    }

    #[test]
    fn missing_monitors_aired_episodes_without_a_file() {
        let eps = fixture();
        // S00E01 (aired, no file), S01E02 + S02E01 (aired, missing) — but NOT
        // S01E01 (aired but on disk) nor S02E02 (not aired).
        assert_eq!(
            MonitorOption::Missing.monitored_flags(&eps),
            vec![true, false, true, true, false]
        );
    }

    #[test]
    fn first_season_monitors_only_the_first_regular_season() {
        let eps = fixture();
        // Season 1 episodes only; the special (S0) and S2 are excluded.
        assert_eq!(
            MonitorOption::FirstSeason.monitored_flags(&eps),
            vec![false, true, true, false, false]
        );
    }

    #[test]
    fn last_season_monitors_only_the_highest_regular_season() {
        let eps = fixture();
        // Season 2 episodes only.
        assert_eq!(
            MonitorOption::LastSeason.monitored_flags(&eps),
            vec![false, false, false, true, true]
        );
    }

    #[test]
    fn pilot_monitors_only_s01e01() {
        let eps = fixture();
        assert_eq!(
            MonitorOption::Pilot.monitored_flags(&eps),
            vec![false, true, false, false, false]
        );
    }

    #[test]
    fn season_bounds_ignore_specials() {
        // A series whose only "season 0" content must not let specials become the
        // first/last season target.
        let eps = vec![
            EpisodeFacts {
                season: 0,
                episode: 1,
                aired: true,
                has_file: false,
            },
            EpisodeFacts {
                season: 3,
                episode: 1,
                aired: true,
                has_file: false,
            },
        ];
        // FirstSeason and LastSeason both resolve to season 3 (the only regular
        // season), never season 0.
        assert_eq!(
            MonitorOption::FirstSeason.monitored_flags(&eps),
            vec![false, true]
        );
        assert_eq!(
            MonitorOption::LastSeason.monitored_flags(&eps),
            vec![false, true]
        );
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert!(MonitorOption::All.monitored_flags(&[]).is_empty());
        // The season-relative options must not panic on an empty set.
        assert!(MonitorOption::FirstSeason.monitored_flags(&[]).is_empty());
        assert!(MonitorOption::LastSeason.monitored_flags(&[]).is_empty());
    }

    #[test]
    fn option_deserializes_from_v3_camelcase_spellings() {
        // The wire spellings the v3 addOptions.monitor field carries.
        let cases = [
            ("\"all\"", MonitorOption::All),
            ("\"existing\"", MonitorOption::Existing),
            ("\"future\"", MonitorOption::Future),
            ("\"missing\"", MonitorOption::Missing),
            ("\"firstSeason\"", MonitorOption::FirstSeason),
            ("\"lastSeason\"", MonitorOption::LastSeason),
            ("\"pilot\"", MonitorOption::Pilot),
            ("\"none\"", MonitorOption::None),
        ];
        for (json, expected) in cases {
            let got: MonitorOption = serde_json::from_str(json).unwrap();
            assert_eq!(got, expected, "deserializing {json}");
        }
    }
}
