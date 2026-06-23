//! Deriving the working quality of a candidate and of an on-disk file.
//!
//! `cellarr-core` owns the quality vocabulary and the mapping itself:
//! [`cellarr_core::QualityRanking`] is the worst→best catalogue and
//! [`cellarr_core::resolve_quality`] turns a parsed release's (source,
//! resolution) into a [`cellarr_core::Quality`] (a name plus its authoritative
//! `rank`). The decision engine no longer keeps its own ranking table; it ranks
//! against core's so the catalogue is single-sourced.
//!
//! This module is the thin bridge from those core types into the decision
//! engine's working state: resolving a candidate to an [`OnDiskFile`]-comparable
//! quality, and projecting a persisted [`cellarr_core::MediaFile`] onto the
//! quality rank + custom-format score that [`crate::decide`] compares.

use cellarr_core::{MediaFile, ParsedRelease, Quality, QualityRanking};

use crate::OnDiskFile;

/// The catalogue name core assigns to a parse it cannot bucket.
///
/// [`cellarr_core::resolve_quality`] always returns a [`Quality`]; when the parse
/// lacks the source/resolution needed to name a bucket it returns this sentinel
/// (rank 0). The decision engine treats that as "no resolvable quality", which is
/// not an allowed quality.
const UNKNOWN_QUALITY_NAME: &str = "Unknown";

/// Resolve a candidate parse to its [`Quality`] against `ranking`, returning
/// `None` when the parse cannot be bucketed (core's `Unknown` sentinel).
///
/// The decision engine treats `None` as a disallowed quality (precedence rule 1).
#[must_use]
pub fn resolve_candidate_quality(
    parsed: &ParsedRelease,
    ranking: &QualityRanking,
) -> Option<Quality> {
    let quality = cellarr_core::resolve_quality(parsed, ranking);
    if quality.name.eq_ignore_ascii_case(UNKNOWN_QUALITY_NAME) {
        None
    } else {
        Some(quality)
    }
}

/// Project a persisted [`MediaFile`] onto the [`OnDiskFile`] the decision engine
/// compares: its quality rank and its custom-format score.
///
/// A file with no recorded custom-format score (`custom_format_score == None`,
/// i.e. it has not yet been scored) is treated as score 0, the neutral total.
#[must_use]
pub fn on_disk_from_media_file(file: &MediaFile) -> OnDiskFile {
    OnDiskFile {
        file_id: file.id,
        quality_rank: file.quality.rank,
        custom_format_score: file.custom_format_score.unwrap_or(0),
        // Carry the persisted release type through so the decision can recognize
        // an already-held full-season pack without re-parsing any title.
        release_type: file.release_type,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::{MediaFileId, Resolution, Source};

    fn parsed(source: Option<Source>, resolution: Option<Resolution>) -> ParsedRelease {
        let mut p = ParsedRelease::new("t");
        p.source = source;
        p.resolution = resolution;
        p
    }

    #[test]
    fn resolve_candidate_quality_ranks_against_core() {
        let ranking = QualityRanking::default();
        let q = resolve_candidate_quality(
            &parsed(Some(Source::Bluray), Some(Resolution::R1080p)),
            &ranking,
        )
        .expect("bluray-1080p resolves");
        assert_eq!(q.name, "Bluray-1080p");
        // The rank must match core's catalogue, not a crate-local table.
        assert_eq!(q.rank, ranking.by_name("Bluray-1080p").unwrap().rank);
    }

    #[test]
    fn unresolvable_parse_is_none() {
        let ranking = QualityRanking::default();
        // No source and no resolution -> core returns the Unknown sentinel, which
        // we map to None. (A resolution-only parse now resolves to HDTV-<res> per
        // parity G1, so it is no longer the "unresolvable" case.)
        assert!(resolve_candidate_quality(&parsed(None, None), &ranking).is_none());
    }

    #[test]
    fn resolution_only_resolves_to_hdtv() {
        let ranking = QualityRanking::default();
        let q = resolve_candidate_quality(&parsed(None, Some(Resolution::R1080p)), &ranking)
            .expect("resolution-only resolves to HDTV-1080p (parity G1)");
        assert_eq!(q.name, "HDTV-1080p");
    }

    #[test]
    fn on_disk_from_media_file_carries_rank_and_score() {
        let ranking = QualityRanking::default();
        let quality = ranking.by_name("Bluray-2160p").expect("present");
        let file = MediaFile {
            id: MediaFileId::new(),
            path: "/library/movie.mkv".to_string(),
            size: 42,
            quality: quality.clone(),
            languages: vec![],
            media_info: None,
            custom_format_score: Some(125),
            release_type: None,
        };
        let on_disk = on_disk_from_media_file(&file);
        assert_eq!(on_disk.file_id, file.id);
        assert_eq!(on_disk.quality_rank, quality.rank);
        assert_eq!(on_disk.custom_format_score, 125);
    }

    #[test]
    fn unscored_media_file_defaults_to_neutral_zero() {
        let ranking = QualityRanking::default();
        let file = MediaFile {
            id: MediaFileId::new(),
            path: "/library/movie.mkv".to_string(),
            size: 42,
            quality: ranking.by_name("Bluray-1080p").unwrap(),
            languages: vec![],
            media_info: None,
            custom_format_score: None,
            release_type: Some(cellarr_core::ReleaseType::FullSeason),
        };
        let projected = on_disk_from_media_file(&file);
        assert_eq!(projected.custom_format_score, 0);
        // The persisted release type is carried onto the OnDiskFile the decision
        // engine compares (so it can recognize an already-held full-season pack).
        assert_eq!(
            projected.release_type,
            Some(cellarr_core::ReleaseType::FullSeason)
        );
    }
}
