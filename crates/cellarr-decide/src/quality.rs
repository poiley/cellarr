//! Mapping a parsed release to its position in the global quality ranking.
//!
//! `cellarr-core`'s [`QualityDefinition`] owns the *ranking* (a name and a `rank`),
//! and [`QualityProfile`] references qualities by `rank`. But core carries no
//! rule for turning a parsed release's (resolution, source) into a quality —
//! that mapping is the decision engine's job. [`QualityResolver`] is the
//! crate-local table that performs it.
//!
//! The mapping mirrors the *arr stack's quality taxonomy clean-room: a quality is
//! identified by a (source, resolution) pair (e.g. Bluray + 1080p ->
//! "Bluray-1080p"), with Remux treated as a distinct, higher source than
//! encoded Bluray.

use cellarr_core::{QualityDefinition, Resolution, Source};

use crate::OnDiskFile;

/// A resolver from a parsed release's facts to a quality `rank`.
///
/// Built from the global [`QualityDefinition`] list plus a (source, resolution)
/// keying rule. Resolution is the dominant axis (a 2160p anything outranks a
/// 1080p anything in the default ranking), with source breaking ties within a
/// resolution.
#[derive(Debug, Clone)]
pub struct QualityResolver {
    entries: Vec<QualityEntry>,
}

#[derive(Debug, Clone)]
struct QualityEntry {
    source: Source,
    resolution: Resolution,
    rank: u32,
}

/// What a candidate's (source, resolution) resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedQuality {
    /// The matched quality's rank in the global ordering.
    pub rank: u32,
}

impl QualityResolver {
    /// Build a resolver from explicit (source, resolution, name) rows, looking up
    /// each name's `rank` in `definitions`. Rows whose name is absent from
    /// `definitions` are skipped, so the ranking stays the single source of truth.
    #[must_use]
    pub fn new(definitions: &[QualityDefinition], rows: &[(Source, Resolution, &str)]) -> Self {
        let mut entries = Vec::new();
        for (source, resolution, name) in rows {
            if let Some(def) = definitions.iter().find(|d| d.name == *name) {
                entries.push(QualityEntry {
                    source: *source,
                    resolution: *resolution,
                    rank: def.rank,
                });
            }
        }
        Self { entries }
    }

    /// The default TRaSH-compatible ranking and (source, resolution) mapping.
    ///
    /// This is the clean-room equivalent of the *arr stack's default quality
    /// list, ordered worst -> best. It is deliberately data here (not hard-coded
    /// constants elsewhere) so tests and the corpus can reference exact ranks.
    #[must_use]
    pub fn default_ranking() -> (Vec<QualityDefinition>, Self) {
        // (name, rank) worst -> best.
        let names: &[(&str, u32)] = &[
            ("CAM", 0),
            ("SDTV", 1),
            ("DVD", 2),
            ("HDTV-720p", 3),
            ("WEBRip-720p", 4),
            ("WEBDL-720p", 5),
            ("Bluray-720p", 6),
            ("HDTV-1080p", 7),
            ("WEBRip-1080p", 8),
            ("WEBDL-1080p", 9),
            ("Bluray-1080p", 10),
            ("Remux-1080p", 11),
            ("HDTV-2160p", 12),
            ("WEBRip-2160p", 13),
            ("WEBDL-2160p", 14),
            ("Bluray-2160p", 15),
            ("Remux-2160p", 16),
        ];
        let definitions: Vec<QualityDefinition> = names
            .iter()
            .map(|(name, rank)| QualityDefinition {
                name: (*name).to_string(),
                rank: *rank,
                min_size_per_min: None,
                max_size_per_min: None,
            })
            .collect();

        use Resolution::{R1080p, R2160p, R720p};
        use Source::{Bluray, Cam, Dvd, Hdtv, Remux, Sdtv, WebDl, Webrip};
        let rows: &[(Source, Resolution, &str)] = &[
            (Cam, Resolution::R480p, "CAM"),
            (Sdtv, Resolution::R480p, "SDTV"),
            (Sdtv, Resolution::R576p, "SDTV"),
            (Dvd, Resolution::R480p, "DVD"),
            (Dvd, Resolution::R576p, "DVD"),
            (Hdtv, R720p, "HDTV-720p"),
            (Webrip, R720p, "WEBRip-720p"),
            (WebDl, R720p, "WEBDL-720p"),
            (Bluray, R720p, "Bluray-720p"),
            (Hdtv, R1080p, "HDTV-1080p"),
            (Webrip, R1080p, "WEBRip-1080p"),
            (WebDl, R1080p, "WEBDL-1080p"),
            (Bluray, R1080p, "Bluray-1080p"),
            (Remux, R1080p, "Remux-1080p"),
            (Hdtv, R2160p, "HDTV-2160p"),
            (Webrip, R2160p, "WEBRip-2160p"),
            (WebDl, R2160p, "WEBDL-2160p"),
            (Bluray, R2160p, "Bluray-2160p"),
            (Remux, R2160p, "Remux-2160p"),
        ];
        let resolver = Self::new(&definitions, rows);
        (definitions, resolver)
    }

    /// Resolve a (source, resolution) pair to a quality rank, or `None` when the
    /// pair is unknown (which the decision engine treats as a disallowed quality).
    #[must_use]
    pub fn resolve(
        &self,
        source: Option<Source>,
        resolution: Option<Resolution>,
    ) -> Option<ResolvedQuality> {
        let source = source?;
        let resolution = resolution?;
        self.entries
            .iter()
            .find(|e| e.source == source && e.resolution == resolution)
            .map(|e| ResolvedQuality { rank: e.rank })
    }
}

/// Convenience: build an [`OnDiskFile`] from a (source, resolution, cf score),
/// resolving the quality through this resolver. Returns `None` if the quality is
/// unknown.
#[must_use]
pub fn on_disk_from_quality(
    resolver: &QualityResolver,
    source: Option<Source>,
    resolution: Option<Resolution>,
    custom_format_score: i32,
    file_id: cellarr_core::MediaFileId,
) -> Option<OnDiskFile> {
    resolver.resolve(source, resolution).map(|q| OnDiskFile {
        file_id,
        quality_rank: q.rank,
        custom_format_score,
    })
}
