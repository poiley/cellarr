//! The source-agnostic intermediate model.
//!
//! Reading a Sonarr or Radarr database produces a [`MappedInstall`]: cellarr
//! domain types (libraries, content nodes, media files, profiles, custom
//! formats, config) plus the little extra identity/linkage the destination
//! schema needs but the slim core handles do not carry (external ids, the
//! file→content links, the typed-metadata side rows).
//!
//! Keeping this as one plain-data value is what makes the importer previewable
//! and reversible: [`crate::preview`] reads it and summarizes; [`crate::import`]
//! reads it and writes. Nothing here touches a database, so it is trivially
//! testable and the same mapping is exercised by preview and import alike.

use cellarr_core::{
    ContentNode, CustomFormat, DownloadClientConfig, IndexerConfig, Library, MediaFile, MediaType,
    QualityProfile, RootFolder,
};

use crate::source::SourceKind;

/// External identifiers carried from the source for a content node, so cellarr
/// never has to re-identify the item against a metadata provider.
///
/// All fields are optional: a given install may know a TMDB id but not an IMDB
/// id, etc. These land in the typed `*_meta` side-tables on import.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExternalIds {
    /// TheMovieDB id (movies and, for newer Sonarr, series).
    pub tmdb_id: Option<i64>,
    /// TheTVDB id (series).
    pub tvdb_id: Option<i64>,
    /// IMDB id (`tt…`).
    pub imdb_id: Option<String>,
}

impl ExternalIds {
    /// Whether any identifier is present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tmdb_id.is_none() && self.tvdb_id.is_none() && self.imdb_id.is_none()
    }
}

/// A mapped content node together with the identity/title metadata the typed
/// side-tables persist (which the slim [`ContentNode`] deliberately omits).
#[derive(Debug, Clone, PartialEq)]
pub struct MappedContent {
    /// The structural node written to the `content` table.
    pub node: ContentNode,
    /// Display title (movie/series/episode title), for the typed `*_meta` row.
    pub title: String,
    /// Release/first-air year, when known.
    pub year: Option<i64>,
    /// External identifiers preserved from the source.
    pub external_ids: ExternalIds,
}

/// A mapped media file plus the content node(s) it satisfies.
///
/// The `content_index` values index into [`MappedInstall::contents`]; resolving
/// them to real [`cellarr_core::ContentId`]s happens at write time. A file that
/// satisfies several nodes (a multi-episode file) lists several indices.
#[derive(Debug, Clone, PartialEq)]
pub struct MappedFile {
    /// The physical file written to the `media_file` table.
    pub file: MediaFile,
    /// Indices into [`MappedInstall::contents`] this file is linked to.
    pub content_indices: Vec<usize>,
}

/// Everything one source database maps to, in cellarr's vocabulary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MappedInstall {
    /// Which app this came from (movies vs TV libraries).
    pub kind: Option<SourceKind>,
    /// The library/libraries (Radarr → one movie library; Sonarr → one TV
    /// library).
    pub libraries: Vec<Library>,
    /// All content nodes, parents before children (a series precedes its
    /// seasons, which precede their episodes), so a single ordered insert
    /// satisfies the `content.parent_id` foreign key.
    pub contents: Vec<MappedContent>,
    /// Media files with their content links.
    pub files: Vec<MappedFile>,
    /// Mapped quality profiles.
    pub profiles: Vec<QualityProfile>,
    /// Mapped custom formats.
    pub custom_formats: Vec<CustomFormat>,
    /// Configured indexers.
    pub indexers: Vec<IndexerConfig>,
    /// Configured download clients.
    pub download_clients: Vec<DownloadClientConfig>,
    /// Configured root folders.
    pub root_folders: Vec<RootFolder>,
}

impl MappedInstall {
    /// Fold another mapped install into this one, so importing a Radarr DB and a
    /// Sonarr DB yields **one** unified library set (movies + TV side by side),
    /// as the migration spec requires.
    ///
    /// `contents` indices in `other.files` are rebased onto this install's
    /// `contents` vector so the file→content links stay correct after merging.
    pub fn merge(&mut self, mut other: MappedInstall) {
        let content_offset = self.contents.len();
        for f in &mut other.files {
            for idx in &mut f.content_indices {
                *idx += content_offset;
            }
        }
        self.libraries.append(&mut other.libraries);
        self.contents.append(&mut other.contents);
        self.files.append(&mut other.files);
        self.profiles.append(&mut other.profiles);
        self.custom_formats.append(&mut other.custom_formats);
        self.indexers.append(&mut other.indexers);
        self.download_clients.append(&mut other.download_clients);
        self.root_folders.append(&mut other.root_folders);
        // A merged install spans media types; record None so callers don't read
        // a single kind off it.
        if self.kind != other.kind {
            self.kind = None;
        }
    }

    /// The media types present across all mapped libraries, for the preview.
    #[must_use]
    pub fn media_types(&self) -> Vec<MediaType> {
        let mut types: Vec<MediaType> = self.libraries.iter().map(|l| l.media_type).collect();
        types.sort_by_key(|t| format!("{t:?}"));
        types.dedup();
        types
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::{
        ContentId, ContentKind, ContentNode, Coordinates, LibraryId, MediaFile, MediaFileId,
        Quality,
    };

    fn one_movie_install() -> MappedInstall {
        let library_id = LibraryId::new();
        let node = ContentNode {
            id: ContentId::new(),
            library_id,
            media_type: MediaType::Movie,
            parent_id: None,
            kind: ContentKind::Movie,
            coords: Coordinates::Movie,
            monitored: true,
            title_id: None,
        };
        MappedInstall {
            kind: None,
            contents: vec![MappedContent {
                node,
                title: "M".to_string(),
                year: None,
                external_ids: ExternalIds::default(),
            }],
            files: vec![MappedFile {
                file: MediaFile {
                    id: MediaFileId::new(),
                    path: "/x.mkv".to_string(),
                    size: 1,
                    quality: Quality::new("Bluray-1080p", 14),
                    languages: vec![],
                    media_info: None,
                    custom_format_score: None,
                },
                // Links to content index 0 within its own install.
                content_indices: vec![0],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn merge_rebases_file_content_indices() {
        let mut a = one_movie_install();
        let b = one_movie_install();
        a.merge(b);

        assert_eq!(a.contents.len(), 2);
        assert_eq!(a.files.len(), 2);
        // The first file still points at content 0; the second must have been
        // rebased to point at content 1 (the merged install's index), not 0.
        assert_eq!(a.files[0].content_indices, vec![0]);
        assert_eq!(a.files[1].content_indices, vec![1]);
        // Every file index resolves to a real content node.
        for f in &a.files {
            for idx in &f.content_indices {
                assert!(a.contents.get(*idx).is_some());
            }
        }
    }
}
