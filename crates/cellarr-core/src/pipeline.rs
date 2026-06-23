//! The acquisition pipeline state machine.
//!
//! One media-type-agnostic state machine drives every acquisition. This module
//! owns the *rules* — the [`Stage`] enum, the legal [`Transition`]s, and the
//! pure logic that validates them. Execution and scheduling live in
//! `cellarr-jobs`; this is deliberately side-effect-free so the rules are
//! trivially testable. See `docs/03-pipeline.md`.

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// The stages an acquisition moves through.
///
/// The happy path is
/// `Discover → Parse → Identify → Decide → Grab → Track → Import → Rename →
/// Notify`. Several stages may also move to a terminal state when the candidate
/// is rejected or a failure is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// A need surfaced (RSS tick, manual search, missing-item scan).
    Discover,
    /// Parse a candidate's release title.
    Parse,
    /// Identify which content node(s) a parsed candidate satisfies.
    Identify,
    /// Decide whether to grab, upgrade, or reject.
    Decide,
    /// Hand a chosen release to a download client.
    Grab,
    /// Track the download to completion.
    Track,
    /// Place completed files into the library (stage→verify→commit→log).
    Import,
    /// Apply final on-disk names.
    Rename,
    /// Notify the user and push to the UI.
    Notify,
    /// Terminal success: the run completed.
    Done,
    /// Terminal: the candidate was rejected at Decide. Not a failure — a normal,
    /// logged outcome.
    Rejected,
    /// Terminal: the run failed and was logged. The originating stage is carried
    /// so the log records where it failed.
    Failed,
    /// Non-terminal: held for user confirmation (low-confidence/destructive).
    HeldForReview,
}

impl Stage {
    /// Whether no further transition is possible from this stage.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Stage::Done | Stage::Rejected | Stage::Failed)
    }
}

/// A requested move from one stage to another, with the kind of move it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    /// The stage being left.
    pub from: Stage,
    /// The stage being entered.
    pub to: Stage,
    /// Whether this is the happy-path advance, a rejection, or a failure.
    pub kind: TransitionKind,
}

/// The category of a [`Transition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    /// The normal forward advance to the next stage.
    Advance,
    /// A rejection at Decide (a normal, logged outcome).
    Reject,
    /// A failure transition (parse-failed, grab-failed, import-failed, …).
    Fail,
    /// Holding for user review (e.g. ambiguous identify, low-confidence import).
    Hold,
    /// Resuming from a held state back to the stage that held it.
    Resume,
}

impl Stage {
    /// The next stage on the happy path, or `None` if this stage has no
    /// forward advance.
    #[must_use]
    pub const fn next(self) -> Option<Stage> {
        Some(match self {
            Stage::Discover => Stage::Parse,
            Stage::Parse => Stage::Identify,
            Stage::Identify => Stage::Decide,
            Stage::Decide => Stage::Grab,
            Stage::Grab => Stage::Track,
            Stage::Track => Stage::Import,
            Stage::Import => Stage::Rename,
            Stage::Rename => Stage::Notify,
            Stage::Notify => Stage::Done,
            Stage::Done | Stage::Rejected | Stage::Failed | Stage::HeldForReview => return None,
        })
    }
}

/// Whether a transition is legal, independent of constructing it.
///
/// Rules:
/// - `Advance` is legal only to [`Stage::next`] of `from`.
/// - `Reject` is legal only from [`Stage::Decide`] to [`Stage::Rejected`].
/// - `Fail` is legal from any non-terminal, non-held stage to [`Stage::Failed`].
/// - `Hold` is legal from `Identify`, `Import`, or `Rename` to
///   [`Stage::HeldForReview`].
/// - `Resume` is legal from [`Stage::HeldForReview`] back to a holdable stage.
#[must_use]
pub fn is_legal_transition(from: Stage, to: Stage, kind: TransitionKind) -> bool {
    match kind {
        TransitionKind::Advance => Stage::next(from) == Some(to),
        TransitionKind::Reject => from == Stage::Decide && to == Stage::Rejected,
        TransitionKind::Fail => {
            !from.is_terminal() && from != Stage::HeldForReview && to == Stage::Failed
        }
        TransitionKind::Hold => is_holdable(from) && to == Stage::HeldForReview,
        TransitionKind::Resume => from == Stage::HeldForReview && is_holdable(to),
    }
}

/// Stages whose work can be held for user review.
const fn is_holdable(stage: Stage) -> bool {
    matches!(stage, Stage::Identify | Stage::Import | Stage::Rename)
}

impl Transition {
    /// Construct a validated transition.
    ///
    /// # Errors
    /// Returns [`CoreError::IllegalTransition`] when the move is not permitted
    /// by [`is_legal_transition`].
    pub fn new(from: Stage, to: Stage, kind: TransitionKind) -> Result<Self, CoreError> {
        if is_legal_transition(from, to, kind) {
            Ok(Self { from, to, kind })
        } else {
            Err(CoreError::IllegalTransition { from, to })
        }
    }

    /// Construct the happy-path advance from `from`, if one exists.
    ///
    /// # Errors
    /// Returns [`CoreError::IllegalTransition`] when `from` has no forward
    /// advance (a terminal or held stage).
    pub fn advance(from: Stage) -> Result<Self, CoreError> {
        let to = Stage::next(from).ok_or(CoreError::IllegalTransition { from, to: from })?;
        Self::new(from, to, TransitionKind::Advance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_advances_in_order() {
        let order = [
            Stage::Discover,
            Stage::Parse,
            Stage::Identify,
            Stage::Decide,
            Stage::Grab,
            Stage::Track,
            Stage::Import,
            Stage::Rename,
            Stage::Notify,
            Stage::Done,
        ];
        for pair in order.windows(2) {
            let t = Transition::advance(pair[0]).expect("advance should be legal");
            assert_eq!(t.to, pair[1]);
            assert_eq!(t.kind, TransitionKind::Advance);
        }
    }

    #[test]
    fn terminal_stages_have_no_advance() {
        for s in [Stage::Done, Stage::Rejected, Stage::Failed] {
            assert!(s.is_terminal());
            assert!(Transition::advance(s).is_err());
        }
    }

    #[test]
    fn skipping_a_stage_is_illegal() {
        // Discover -> Identify (skips Parse) must be rejected.
        let err = Transition::new(Stage::Discover, Stage::Identify, TransitionKind::Advance);
        assert!(err.is_err());
    }

    #[test]
    fn reject_only_from_decide() {
        assert!(Transition::new(Stage::Decide, Stage::Rejected, TransitionKind::Reject).is_ok());
        assert!(Transition::new(Stage::Grab, Stage::Rejected, TransitionKind::Reject).is_err());
        assert!(Transition::new(Stage::Parse, Stage::Rejected, TransitionKind::Reject).is_err());
    }

    #[test]
    fn fail_from_any_active_stage() {
        for s in [
            Stage::Discover,
            Stage::Parse,
            Stage::Identify,
            Stage::Decide,
            Stage::Grab,
            Stage::Track,
            Stage::Import,
            Stage::Rename,
            Stage::Notify,
        ] {
            assert!(
                Transition::new(s, Stage::Failed, TransitionKind::Fail).is_ok(),
                "fail from {s:?} should be legal"
            );
        }
        // Cannot fail from a terminal or held stage.
        assert!(Transition::new(Stage::Done, Stage::Failed, TransitionKind::Fail).is_err());
        assert!(
            Transition::new(Stage::HeldForReview, Stage::Failed, TransitionKind::Fail).is_err()
        );
    }

    #[test]
    fn hold_and_resume_round_trip() {
        for s in [Stage::Identify, Stage::Import, Stage::Rename] {
            let hold = Transition::new(s, Stage::HeldForReview, TransitionKind::Hold);
            assert!(hold.is_ok(), "hold from {s:?} should be legal");
            let resume = Transition::new(Stage::HeldForReview, s, TransitionKind::Resume);
            assert!(resume.is_ok(), "resume to {s:?} should be legal");
        }
        // Non-holdable stage cannot hold.
        assert!(Transition::new(Stage::Grab, Stage::HeldForReview, TransitionKind::Hold).is_err());
    }

    #[test]
    fn wrong_kind_for_target_is_illegal() {
        // Advancing into Failed is not an Advance.
        assert!(Transition::new(Stage::Parse, Stage::Failed, TransitionKind::Advance).is_err());
    }
}
