//! The decision function: grab / upgrade / reject, with a structured reason.
//!
//! Precedence is the contract (see `docs/05-decision-engine.md`). The order here
//! is load-bearing and must not be reordered:
//!
//! 1. Hard rejects (disallowed quality, below min CF score, blocklist, size,
//!    language).
//! 2. Quality rank dominates — never downgrade quality to chase CF score.
//! 3. CF score breaks ties within equal quality.
//! 4. Upgrade gating — only on a real upgrade, with upgrades allowed and
//!    **both** cutoffs (quality and CF score) unmet; stop once both are met.
//! 5. Proper/repack per policy.

use cellarr_core::{
    ContentRef, CustomFormat, Decision, MediaFileId, ParsedRelease, ProperRepack, QualityProfile,
    RejectReason, Release, Score, Verdict,
};

use crate::error::DecideError;
use crate::matching::MatchContext;
use crate::quality::{QualityResolver, ResolvedQuality};
use crate::scoring::score;

/// What is already on disk for the content node under consideration.
///
/// Core has no on-disk-file type carrying a resolved quality rank and CF score
/// (that pairing is the decision engine's working state), so this crate-local
/// type stands in. It is the minimum the decision needs: which file, its quality
/// rank, and its custom-format score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnDiskFile {
    /// The library file this describes (carried into [`Verdict::Upgrade`]).
    pub file_id: MediaFileId,
    /// The file's quality position in the global ranking.
    pub quality_rank: u32,
    /// The file's total custom-format score.
    pub custom_format_score: i32,
}

/// How proper/repack releases are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProperRepackPolicy {
    /// A proper/repack at the *same* quality and CF score is preferred over the
    /// existing file (treated as a real upgrade).
    #[default]
    Prefer,
    /// Proper/repack is never on its own a reason to replace an equal file.
    DoNotPrefer,
}

/// Everything the decision needs beyond the candidate and the on-disk file.
///
/// Bundled so [`decide`] keeps a small signature and callers build it once.
#[derive(Debug, Clone)]
pub struct DecisionContext<'a> {
    /// The active quality profile.
    pub profile: &'a QualityProfile,
    /// The custom formats to score against.
    pub custom_formats: &'a [CustomFormat],
    /// The (source, resolution) -> rank resolver.
    pub resolver: &'a QualityResolver,
    /// Whether the release (or its group) is blocklisted. Blocklist membership
    /// is a repository concern; the caller supplies the verdict.
    pub blocklisted: bool,
    /// Proper/repack handling.
    pub proper_repack_policy: ProperRepackPolicy,
}

/// The candidate's computed standing: quality + CF score, ready to compare.
struct CandidateStanding {
    quality: ResolvedQuality,
    cf_score: i32,
}

/// Decide the verdict for `candidate` given what is `on_disk`.
///
/// # Errors
/// Returns [`DecideError::InvalidRegex`] if a custom-format title pattern fails
/// to compile. All other branches are infallible.
pub fn decide(
    content_ref: ContentRef,
    candidate: &Release,
    parsed: &ParsedRelease,
    on_disk: Option<OnDiskFile>,
    ctx: &DecisionContext<'_>,
) -> Result<Decision, DecideError> {
    let match_ctx = MatchContext::new(ctx.custom_formats)?;
    let cf_score = score(candidate, parsed, ctx.custom_formats, &match_ctx);

    let verdict = decide_verdict(candidate, parsed, cf_score, on_disk, ctx);

    Ok(Decision {
        content_ref,
        release: candidate.clone(),
        verdict,
    })
}

/// The pure verdict computation, factored out so it can be tested without
/// constructing a [`MatchContext`] from scratch each time.
fn decide_verdict(
    candidate: &Release,
    parsed: &ParsedRelease,
    cf_score: i32,
    on_disk: Option<OnDiskFile>,
    ctx: &DecisionContext<'_>,
) -> Verdict {
    // 1. Hard rejects.
    if ctx.blocklisted {
        return reject(RejectReason::Blocklisted);
    }

    let Some(quality) = ctx.resolver.resolve(parsed.source, parsed.resolution) else {
        return reject(RejectReason::QualityNotAllowed);
    };

    if !ctx.profile.allowed_qualities.contains(&quality.rank) {
        return reject(RejectReason::QualityNotAllowed);
    }

    if cf_score < ctx.profile.min_custom_format_score {
        return reject(RejectReason::BelowMinimumCustomFormatScore);
    }

    if let Some(reason) = language_reject(parsed, ctx.profile) {
        return reject(reason);
    }

    let standing = CandidateStanding { quality, cf_score };

    match on_disk {
        // Nothing acceptable on disk: grab.
        None => Verdict::Grab {
            score: standing.as_score(),
        },
        Some(existing) => decide_against_existing(candidate, parsed, &standing, existing, ctx),
    }
}

/// The upgrade vs reject branch when a file already exists.
fn decide_against_existing(
    candidate: &Release,
    parsed: &ParsedRelease,
    standing: &CandidateStanding,
    existing: OnDiskFile,
    ctx: &DecisionContext<'_>,
) -> Verdict {
    let from = Score {
        quality_rank: existing.quality_rank,
        custom_format_score: existing.custom_format_score,
    };
    let to = standing.as_score();

    // 2 + 4: quality rank dominates. A strictly higher quality is always an
    // upgrade (when upgrades are allowed and the quality cutoff is unmet); a
    // strictly lower quality is never an upgrade — we never downgrade quality to
    // chase a higher CF score.
    if standing.quality.rank > existing.quality_rank {
        return if ctx.profile.upgrades_allowed && existing.quality_rank < ctx.profile.cutoff_quality
        {
            Verdict::Upgrade {
                replacing: existing.file_id,
                from,
                to,
            }
        } else if existing.quality_rank >= ctx.profile.cutoff_quality {
            reject(RejectReason::CutoffAlreadyMet)
        } else {
            reject(RejectReason::NotAnUpgrade)
        };
    }

    if standing.quality.rank < existing.quality_rank {
        // Lower quality is never an upgrade regardless of CF score.
        return reject(RejectReason::NotAnUpgrade);
    }

    // 3 + 4: equal quality. CF score breaks the tie, gated by both cutoffs.
    decide_equal_quality(candidate, parsed, standing, existing, from, to, ctx)
}

/// Equal-quality branch: a higher CF score (or a preferred proper/repack) is the
/// only thing that can make this an upgrade, and only while both cutoffs are unmet.
#[allow(clippy::too_many_arguments)]
fn decide_equal_quality(
    candidate: &Release,
    parsed: &ParsedRelease,
    standing: &CandidateStanding,
    existing: OnDiskFile,
    from: Score,
    to: Score,
    ctx: &DecisionContext<'_>,
) -> Verdict {
    let quality_cutoff_met = existing.quality_rank >= ctx.profile.cutoff_quality;
    let cf_cutoff_met =
        existing.custom_format_score >= ctx.profile.upgrade_until_custom_format_score;

    // 4: stop once BOTH cutoffs are met — no churn.
    if quality_cutoff_met && cf_cutoff_met {
        return reject(RejectReason::CutoffAlreadyMet);
    }

    if !ctx.profile.upgrades_allowed {
        return reject(RejectReason::NotAnUpgrade);
    }

    // A genuinely higher CF score is an upgrade.
    if standing.cf_score > existing.custom_format_score {
        return Verdict::Upgrade {
            replacing: existing.file_id,
            from,
            to,
        };
    }

    // 5: proper/repack policy. At equal quality and equal-or-higher CF score, a
    // proper/repack fixes the existing file and is preferred.
    if standing.cf_score == existing.custom_format_score
        && ctx.proper_repack_policy == ProperRepackPolicy::Prefer
        && is_proper_or_repack(candidate, parsed)
    {
        return Verdict::Upgrade {
            replacing: existing.file_id,
            from,
            to,
        };
    }

    reject(RejectReason::NotAnUpgrade)
}

impl CandidateStanding {
    fn as_score(&self) -> Score {
        Score {
            quality_rank: self.quality.rank,
            custom_format_score: self.cf_score,
        }
    }
}

/// Whether the candidate is a proper or repack, from the parse or a raw-title
/// fallback (the parse is authoritative; the title check is a belt-and-braces
/// guard for parses that missed the marker).
fn is_proper_or_repack(candidate: &Release, parsed: &ParsedRelease) -> bool {
    if matches!(
        parsed.proper_repack,
        Some(ProperRepack::Proper | ProperRepack::Repack)
    ) {
        return true;
    }
    let lower = candidate.title.to_ascii_lowercase();
    lower.contains("proper") || lower.contains("repack")
}

/// A reject verdict if a required language is absent from the parse.
fn language_reject(parsed: &ParsedRelease, profile: &QualityProfile) -> Option<RejectReason> {
    if profile.required_languages.is_empty() {
        return None;
    }
    let all_present = profile.required_languages.iter().all(|required| {
        parsed
            .languages
            .iter()
            .any(|l| l.eq_ignore_ascii_case(required))
    });
    (!all_present).then_some(RejectReason::LanguageRequirementUnmet)
}

fn reject(reason: RejectReason) -> Verdict {
    Verdict::Reject { reason }
}
