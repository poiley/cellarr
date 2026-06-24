//! Delay profiles: hold a grabbable release for a protocol-specific window so a
//! better release can arrive first.
//!
//! A delay profile encodes the Sonarr/Radarr "Delay Profile" behavior: when a
//! release would otherwise be grabbed, the grab is **held** until its protocol's
//! configured delay has elapsed since the release was first seen. During that
//! window a higher-standing release (better quality / custom-format score) can
//! show up and win instead — the whole point of the delay. Two further rules
//! shape it:
//!
//! * **Preferred protocol** breaks ties: when both a usenet and a torrent release
//!   are available, the profile's [`PreferredProtocol`] decides which one is
//!   eligible to grab (the other is held even past its delay until the preferred
//!   one's window also lapses or it disappears). Modeled here as a per-protocol
//!   delay plus a preference flag the decision path consults.
//! * **Bypass on highest quality** ([`bypass_if_highest_quality`]): a release that
//!   is already at the profile's cutoff / highest allowed quality is grabbed
//!   immediately, never delayed — waiting cannot improve on the best.
//!
//! Core owns the *data model* and the pure [`DelayProfile::hold_decision`]
//! arithmetic (given "now", "first seen", and whether the candidate is already at
//! the highest quality, should this release be held?). The jobs runner wires the
//! clock and the first-seen bookkeeping; the decision engine supplies "is this the
//! cutoff/highest quality".

use serde::{Deserialize, Serialize};

use crate::ids::DelayProfileId;
use crate::release::Protocol;

/// Which protocol a delay profile prefers when both are available.
///
/// `Either` expresses no preference — neither protocol is favored, each is gated
/// only by its own delay. `Usenet`/`Torrent` favor that protocol: a release on
/// the non-preferred protocol is held while a preferred-protocol release is still
/// within reach (its delay not yet elapsed), so the preferred one wins ties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreferredProtocol {
    /// No protocol preference; each is gated only by its own delay.
    #[default]
    Either,
    /// Prefer usenet releases over torrents.
    Usenet,
    /// Prefer torrent releases over usenet.
    Torrent,
}

impl PreferredProtocol {
    /// Whether `protocol` is the preferred one. `Either` prefers neither (returns
    /// `false` for both), so callers treat "preferred" as a strict favorite.
    #[must_use]
    pub fn prefers(self, protocol: Protocol) -> bool {
        matches!(
            (self, protocol),
            (PreferredProtocol::Usenet, Protocol::Usenet)
                | (PreferredProtocol::Torrent, Protocol::Torrent)
        )
    }
}

/// What the delay decision says to do with a grabbable candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelayVerdict {
    /// Grab now — no applicable delay, the delay has elapsed, or a bypass applies.
    Grab,
    /// Hold the release; its protocol's delay has not yet elapsed since it was
    /// first seen. Carries the remaining wait in minutes so the caller can log /
    /// re-check it.
    Hold {
        /// Minutes still to wait before this release becomes grabbable.
        remaining_minutes: u32,
    },
}

impl DelayVerdict {
    /// Whether this verdict holds the release (vs. grab-now).
    #[must_use]
    pub fn is_hold(self) -> bool {
        matches!(self, DelayVerdict::Hold { .. })
    }
}

/// A delay profile: per-protocol grab delays plus the preference and bypass
/// rules. Mirrors the Sonarr/Radarr delay profile the ecosystem expects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelayProfile {
    /// Profile identifier.
    pub id: DelayProfileId,
    /// Whether this profile is active. A disabled profile never holds anything.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Which protocol wins ties when both are available.
    #[serde(default)]
    pub preferred_protocol: PreferredProtocol,
    /// Minutes to hold a usenet release after it is first seen.
    #[serde(default)]
    pub usenet_delay: u32,
    /// Minutes to hold a torrent release after it is first seen.
    #[serde(default)]
    pub torrent_delay: u32,
    /// When true, a release already at the cutoff / highest allowed quality is
    /// grabbed immediately, never delayed.
    #[serde(default)]
    pub bypass_if_highest_quality: bool,
    /// The tag ids this profile applies to (empty = the catch-all default
    /// profile, which applies to everything untagged). Stored as opaque strings so
    /// the tag vocabulary can evolve without a model change.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Ordering among profiles; lower applies first (Sonarr's convention). The
    /// catch-all default profile sorts last.
    #[serde(default)]
    pub order: i32,
}

impl DelayProfile {
    /// The catch-all default delay profile: enabled, no preference, no delay, no
    /// tags. Holding nothing, it is a safe baseline the system can always fall
    /// back to when no profile is configured.
    #[must_use]
    pub fn default_catch_all() -> Self {
        Self {
            id: DelayProfileId::new(),
            enabled: true,
            preferred_protocol: PreferredProtocol::Either,
            usenet_delay: 0,
            torrent_delay: 0,
            bypass_if_highest_quality: false,
            tags: Vec::new(),
            order: i32::MAX,
        }
    }

    /// The configured delay (minutes) for `protocol`.
    #[must_use]
    pub fn delay_for(&self, protocol: Protocol) -> u32 {
        match protocol {
            Protocol::Usenet => self.usenet_delay,
            Protocol::Torrent => self.torrent_delay,
        }
    }

    /// Decide whether a grabbable candidate should be **held** under this profile.
    ///
    /// `protocol` is the candidate's protocol; `minutes_since_first_seen` is how
    /// long ago the release was first observed; `is_highest_quality` is whether the
    /// candidate already sits at the profile-cutoff / highest allowed quality (the
    /// decision engine computes this).
    ///
    /// Rules, in order:
    /// 1. A disabled profile (or a zero delay for the protocol) never holds.
    /// 2. `bypass_if_highest_quality` + a highest-quality candidate grabs now.
    /// 3. Otherwise hold until `minutes_since_first_seen >= delay_for(protocol)`.
    #[must_use]
    pub fn hold_decision(
        &self,
        protocol: Protocol,
        minutes_since_first_seen: u32,
        is_highest_quality: bool,
    ) -> DelayVerdict {
        if !self.enabled {
            return DelayVerdict::Grab;
        }
        let delay = self.delay_for(protocol);
        if delay == 0 {
            return DelayVerdict::Grab;
        }
        if self.bypass_if_highest_quality && is_highest_quality {
            return DelayVerdict::Grab;
        }
        if minutes_since_first_seen >= delay {
            DelayVerdict::Grab
        } else {
            DelayVerdict::Hold {
                remaining_minutes: delay - minutes_since_first_seen,
            }
        }
    }

    /// Whether this profile applies to a content node carrying `content_tags`.
    ///
    /// A tagless profile is the catch-all default and applies to everything; a
    /// tagged profile applies only when it shares at least one tag with the node
    /// (case-insensitively, matching the rest of cellarr's tag handling).
    #[must_use]
    pub fn applies_to(&self, content_tags: &[String]) -> bool {
        if self.tags.is_empty() {
            return true;
        }
        self.tags
            .iter()
            .any(|t| content_tags.iter().any(|c| c.eq_ignore_ascii_case(t)))
    }
}

/// Pick the delay profile that governs a content node from a set of profiles.
///
/// Mirrors Sonarr/Radarr: the **most specific** enabled profile wins — a tagged
/// profile that shares a tag with the node beats the tagless catch-all. Among
/// equally-applicable profiles, the lowest [`order`](DelayProfile::order) wins.
/// Returns `None` only when `profiles` is empty (a deployment with no profiles at
/// all imposes no delay).
#[must_use]
pub fn resolve_delay_profile<'a>(
    profiles: &'a [DelayProfile],
    content_tags: &[String],
) -> Option<&'a DelayProfile> {
    profiles
        .iter()
        .filter(|p| p.enabled && p.applies_to(content_tags))
        // A tagged (specific) profile outranks the tagless catch-all; within the
        // same specificity, the lower order wins.
        .min_by_key(|p| (p.tags.is_empty(), p.order))
}

/// The serde default for the `enabled` flag.
const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> DelayProfile {
        DelayProfile {
            id: DelayProfileId::new(),
            enabled: true,
            preferred_protocol: PreferredProtocol::Usenet,
            usenet_delay: 30,
            torrent_delay: 60,
            bypass_if_highest_quality: true,
            tags: Vec::new(),
            order: 1,
        }
    }

    #[test]
    fn holds_until_delay_elapses() {
        let p = profile();
        // 10 < 30 -> hold, 20 remaining.
        assert_eq!(
            p.hold_decision(Protocol::Usenet, 10, false),
            DelayVerdict::Hold {
                remaining_minutes: 20
            }
        );
        // 30 >= 30 -> grab.
        assert_eq!(
            p.hold_decision(Protocol::Usenet, 30, false),
            DelayVerdict::Grab
        );
        // Past the delay -> grab.
        assert_eq!(
            p.hold_decision(Protocol::Usenet, 45, false),
            DelayVerdict::Grab
        );
    }

    #[test]
    fn per_protocol_delay() {
        let p = profile();
        // Torrent uses the 60-minute delay; 30 < 60 -> still held.
        assert_eq!(
            p.hold_decision(Protocol::Torrent, 30, false),
            DelayVerdict::Hold {
                remaining_minutes: 30
            }
        );
    }

    #[test]
    fn bypass_on_highest_quality_grabs_immediately() {
        let p = profile();
        // Within the window but highest quality + bypass -> grab now.
        assert_eq!(
            p.hold_decision(Protocol::Usenet, 0, true),
            DelayVerdict::Grab
        );
        // Without the highest-quality flag the same release is held.
        assert!(p.hold_decision(Protocol::Usenet, 0, false).is_hold());
    }

    #[test]
    fn zero_delay_never_holds() {
        let mut p = profile();
        p.usenet_delay = 0;
        assert_eq!(
            p.hold_decision(Protocol::Usenet, 0, false),
            DelayVerdict::Grab
        );
    }

    #[test]
    fn disabled_profile_never_holds() {
        let mut p = profile();
        p.enabled = false;
        assert_eq!(
            p.hold_decision(Protocol::Usenet, 0, false),
            DelayVerdict::Grab
        );
    }

    #[test]
    fn preferred_protocol_predicate() {
        assert!(PreferredProtocol::Usenet.prefers(Protocol::Usenet));
        assert!(!PreferredProtocol::Usenet.prefers(Protocol::Torrent));
        assert!(PreferredProtocol::Torrent.prefers(Protocol::Torrent));
        // Either prefers neither.
        assert!(!PreferredProtocol::Either.prefers(Protocol::Usenet));
        assert!(!PreferredProtocol::Either.prefers(Protocol::Torrent));
    }

    #[test]
    fn resolve_prefers_tagged_then_order() {
        let mut catch_all = DelayProfile::default_catch_all();
        catch_all.order = 0;
        let mut tagged = profile();
        tagged.tags = vec!["anime".into()];
        tagged.order = 5;
        let profiles = vec![catch_all.clone(), tagged.clone()];

        // A node tagged "anime" gets the tagged profile despite its higher order
        // (specificity beats order).
        let picked = resolve_delay_profile(&profiles, &["Anime".into()]).expect("a profile");
        assert_eq!(picked.id, tagged.id);

        // An untagged node falls to the catch-all.
        let picked2 = resolve_delay_profile(&profiles, &[]).expect("a profile");
        assert_eq!(picked2.id, catch_all.id);
    }

    #[test]
    fn preferred_protocol_wins_ties_via_asymmetric_delays() {
        // The preferred protocol winning ties is expressed as an asymmetric delay:
        // the preferred protocol is grabbable sooner (or immediately) while the
        // non-preferred one is still held, so when two equal-standing releases
        // arrive together the preferred one is grabbed first. Here usenet is
        // preferred (no usenet delay) and torrent is held 60 min.
        let mut p = profile();
        p.preferred_protocol = PreferredProtocol::Usenet;
        p.usenet_delay = 0;
        p.torrent_delay = 60;
        // At the same instant (t=10) the usenet release is free while the torrent
        // is still held -> the preferred protocol wins the tie.
        assert_eq!(
            p.hold_decision(Protocol::Usenet, 10, false),
            DelayVerdict::Grab
        );
        assert!(p.hold_decision(Protocol::Torrent, 10, false).is_hold());
        assert!(p.preferred_protocol.prefers(Protocol::Usenet));
    }

    #[test]
    fn applies_to_is_case_insensitive() {
        let mut p = profile();
        p.tags = vec!["HD".into()];
        assert!(p.applies_to(&["hd".into()]));
        assert!(!p.applies_to(&["uhd".into()]));
        // Tagless applies to everything.
        p.tags.clear();
        assert!(p.applies_to(&[]));
    }
}
