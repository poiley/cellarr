//! Recognize-in-place planning.
//!
//! The migration spec's hardest safety rule: importing an existing library must
//! schedule **zero** file operations for files that are already where they should
//! be (docs/12-migration.md). Migration never moves or deletes; the destructive
//! pipeline only runs on *future* grabs. This module makes that auditable.
//!
//! [`plan_file_operations`] diffs each imported file's current on-disk path
//! against the path a naming policy *would* place it at. When they match (the
//! recognize-in-place case) it emits nothing. A non-matching policy — which
//! migration does **not** use, but which a later "rename my library" feature
//! would — is what produces [`PlannedMove`]s, so the same function serves both
//! and the no-op property is provable rather than asserted by construction.

use cellarr_core::decision::PlannedMove;

use crate::model::MappedInstall;

/// A naming policy: given a file's current path, return the path it *should*
/// occupy. The recognize-in-place policy is the identity function.
pub trait NamingPolicy {
    /// The desired absolute path for a file currently at `current_path`.
    fn desired_path(&self, current_path: &str) -> String;
}

/// The policy migration always uses: keep every file exactly where it is.
///
/// With this policy [`plan_file_operations`] is guaranteed to return an empty
/// plan, which is the recognize-in-place guarantee in code form.
#[derive(Debug, Clone, Copy, Default)]
pub struct RecognizeInPlace;

impl NamingPolicy for RecognizeInPlace {
    fn desired_path(&self, current_path: &str) -> String {
        current_path.to_string()
    }
}

impl<F> NamingPolicy for F
where
    F: Fn(&str) -> String,
{
    fn desired_path(&self, current_path: &str) -> String {
        self(current_path)
    }
}

/// Compute the file operations needed to reconcile an imported install with a
/// naming `policy`.
///
/// A file already at its desired path yields no [`PlannedMove`]. Migration calls
/// this with [`RecognizeInPlace`], so the result is always empty — the
/// recognize-in-place invariant. The function is shared so a future rename
/// feature can reuse it and the invariant stays testable.
#[must_use]
pub fn plan_file_operations<P: NamingPolicy>(
    install: &MappedInstall,
    policy: &P,
) -> Vec<PlannedMove> {
    let mut moves = Vec::new();
    for mapped in &install.files {
        let current = &mapped.file.path;
        let desired = policy.desired_path(current);
        if desired == *current {
            // Already in place: no operation. This is the migration case.
            continue;
        }
        let content_ids = mapped
            .content_indices
            .iter()
            .filter_map(|idx| install.contents.get(*idx))
            .map(|c| c.node.id)
            .collect();
        moves.push(PlannedMove {
            source_path: current.clone(),
            destination_path: desired,
            content_ids,
            replaces: None,
            replaced_path: None,
            // Recognizing/relocating within the same library is a hardlink-able
            // move on the common single-filesystem layout.
            hardlink: true,
        });
    }
    moves
}

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::{
        ContentId, ContentKind, ContentNode, Coordinates, LibraryId, MediaFile, MediaFileId,
        MediaType, Quality,
    };

    use crate::model::{ExternalIds, MappedContent, MappedFile};

    fn install_with_one_file(path: &str) -> MappedInstall {
        let library_id = LibraryId::new();
        let node = ContentNode {
            tags: Vec::new(),
            id: ContentId::new(),
            library_id,
            media_type: MediaType::Movie,
            parent_id: None,
            kind: ContentKind::Movie,
            series_type: cellarr_core::SeriesType::Standard,
            coords: Coordinates::Movie,
            monitored: true,
            title_id: None,
        };
        MappedInstall {
            contents: vec![MappedContent {
                node,
                title: "T".to_string(),
                year: None,
                external_ids: ExternalIds::default(),
            }],
            files: vec![MappedFile {
                file: MediaFile {
                    id: MediaFileId::new(),
                    path: path.to_string(),
                    size: 1,
                    quality: Quality::new("Bluray-1080p", 14),
                    languages: vec![],
                    media_info: None,
                    custom_format_score: None,
                    release_type: None,
                },
                content_indices: vec![0],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn recognize_in_place_schedules_zero_operations() {
        let install = install_with_one_file("/library/movie.mkv");
        let plan = plan_file_operations(&install, &RecognizeInPlace);
        assert!(
            plan.is_empty(),
            "files already in place must yield no operations"
        );
    }

    #[test]
    fn a_renaming_policy_does_produce_a_move() {
        // Proves the no-op result above is a real property of in-place naming,
        // not a planner that can never emit anything.
        let install = install_with_one_file("/library/old.mkv");
        let plan = plan_file_operations(&install, &|p: &str| p.replace("old", "new"));
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].source_path, "/library/old.mkv");
        assert_eq!(plan[0].destination_path, "/library/new.mkv");
        // The move carries the content node it satisfies.
        assert_eq!(plan[0].content_ids.len(), 1);
    }
}
