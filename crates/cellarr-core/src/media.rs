//! The media-type and numbering vocabulary.
//!
//! These are the types that let one pipeline serve movies, TV, music, and books
//! without per-type branching outside [`Coordinates`] and the `MediaModule`
//! trait. See `docs/02-data-model.md`.

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::{ContentId, LibraryId};

/// The four supported media types.
///
/// Serializes in lowercase so it reads naturally in JSON columns and APIs.
///
/// ```
/// # use cellarr_core::MediaType;
/// let json = serde_json::to_string(&MediaType::Movie).unwrap();
/// assert_eq!(json, "\"movie\"");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    /// Films.
    Movie,
    /// Television (series → season → episode).
    Tv,
    /// Music (artist → album → track).
    Music,
    /// Books (author → book).
    Book,
}

/// How a unit of media is addressed within its type.
///
/// This is the one place the media types genuinely differ, so it is named
/// explicitly and modeled as a closed enum. It is stored in the `content.coords`
/// column as **tagged JSON** (`{ "type": "...", ... }`) so it round-trips
/// losslessly and is self-describing.
///
/// ```
/// # use cellarr_core::Coordinates;
/// let c = Coordinates::Episode { season: 2, episode: 15, absolute: None };
/// let json = serde_json::to_value(&c).unwrap();
/// assert_eq!(json["type"], "episode");
/// assert_eq!(json["season"], 2);
/// let back: Coordinates = serde_json::from_value(json).unwrap();
/// assert_eq!(back, c);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Coordinates {
    /// A movie is its own unit; it carries no coordinates.
    Movie,
    /// A television episode, addressed by season and episode, optionally with the
    /// anime absolute number when known.
    Episode {
        /// Season number (specials are conventionally season 0).
        season: u32,
        /// Episode number within the season.
        episode: u32,
        /// Absolute episode number across the whole series (anime numbering).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        absolute: Option<u32>,
    },
    /// A music track, addressed by disc and track number.
    Track {
        /// Disc number (1-based; single-disc albums use 1).
        disc: u32,
        /// Track number within the disc.
        track: u32,
    },
    /// A book, optionally positioned within a series.
    Book {
        /// Position within a series, when the book belongs to one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        series_position: Option<u32>,
    },
}

impl Coordinates {
    /// The media type these coordinates are valid for.
    #[must_use]
    pub const fn media_type(&self) -> MediaType {
        match self {
            Coordinates::Movie => MediaType::Movie,
            Coordinates::Episode { .. } => MediaType::Tv,
            Coordinates::Track { .. } => MediaType::Music,
            Coordinates::Book { .. } => MediaType::Book,
        }
    }

    /// Verify these coordinates are consistent with `media_type`.
    ///
    /// # Errors
    /// Returns [`CoreError::InvalidCoordinates`] if the coordinate variant does
    /// not belong to `media_type`.
    pub fn validate_for(&self, media_type: MediaType) -> Result<(), CoreError> {
        if self.media_type() == media_type {
            Ok(())
        } else {
            Err(CoreError::InvalidCoordinates {
                media_type,
                detail: format!("{self:?} is not addressable in a {media_type:?} library"),
            })
        }
    }
}

/// The small handle the pipeline carries instead of a full media object.
///
/// Anything richer than this (titles, search terms, naming tokens) is obtained
/// by asking the `MediaModule` for the type. The pipeline never branches on
/// `media_type` directly — it delegates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentRef {
    /// The structural `content` node this refers to.
    pub id: ContentId,
    /// The library the node lives in.
    pub library_id: LibraryId,
    /// The media type, carried so callers can pick the right `MediaModule`.
    pub media_type: MediaType,
    /// The node's coordinates within its type.
    pub coords: Coordinates,
}

impl ContentRef {
    /// Construct a reference, validating that `coords` match `media_type`.
    ///
    /// # Errors
    /// Returns [`CoreError::InvalidCoordinates`] when the coordinates do not
    /// belong to the media type.
    pub fn new(
        id: ContentId,
        library_id: LibraryId,
        media_type: MediaType,
        coords: Coordinates,
    ) -> Result<Self, CoreError> {
        coords.validate_for(media_type)?;
        Ok(Self {
            id,
            library_id,
            media_type,
            coords,
        })
    }
}

/// A typed collection of content of a single media type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    /// Library identifier.
    pub id: LibraryId,
    /// The media type every node in this library shares.
    pub media_type: MediaType,
    /// Human-facing name (e.g. "Movies — 4K").
    pub name: String,
    /// Root folders this library imports into.
    pub root_folders: Vec<String>,
    /// The default quality profile applied to new items.
    pub default_quality_profile: crate::ids::QualityProfileId,
}
