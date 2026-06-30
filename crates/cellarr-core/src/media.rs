//! The media-type and numbering vocabulary.
//!
//! These are the types that let one pipeline serve movies, TV, music, and books
//! without per-type branching outside [`Coordinates`] and the `MediaModule`
//! trait. See `docs/02-data-model.md`.

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::{ContentId, LibraryId, MediaFileId, TitleId};
use crate::profile::Quality;

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

/// How a television series is numbered, which selects the addressing behaviour
/// Identify applies to its releases.
///
/// This mirrors Sonarr's `seriesType`: a `Standard` series is addressed by its
/// native season/episode numbering; a `Daily` series is broadcast-date addressed;
/// an `Anime` series is the one that advertises **absolute** episode numbers a
/// release carries, so it is the switch that turns on the absolute→season/episode
/// scene-mapping remap. A non-anime series parsing a bare number is therefore
/// **never** force-remapped — only an `Anime`-typed series reaches the scene-map
/// path (the library-safety rule: do not guess a placement for a standard show).
///
/// Serializes in lowercase to match the v3 (`"standard"`/`"daily"`/`"anime"`)
/// wire shape and the `content.series_type` column.
///
/// ```
/// # use cellarr_core::SeriesType;
/// assert_eq!(serde_json::to_string(&SeriesType::Anime).unwrap(), "\"anime\"");
/// assert_eq!(SeriesType::default(), SeriesType::Standard);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeriesType {
    /// Native season/episode numbering (the default for any non-anime show).
    #[default]
    Standard,
    /// A broadcast-date addressed (daily) show.
    Daily,
    /// An absolute-numbered anime show — turns on the absolute→episode remap.
    Anime,
}

/// How a unit of media is addressed within its type.
///
/// This is the one place the media types genuinely differ, so it is named
/// explicitly and modeled as a closed enum. It is stored in the `content.coords`
/// column as **tagged JSON** (`{ "type": "...", ... }`) so it round-trips
/// losslessly and is self-describing.
///
/// # Which stage produces which variant
///
/// The numbering vocabulary spans two pipeline stages, so several variants are
/// transient. The parser may emit the *advertised* numbering it sees in a
/// release title; Identify then normalizes those to the canonical addressing the
/// rest of the pipeline carries:
///
/// - [`Coordinates::Movie`] / [`Coordinates::Track`] / [`Coordinates::Book`] —
///   canonical for their media type; produced by the parser and carried
///   unchanged.
/// - [`Coordinates::Episode`] — the canonical TV addressing. The parser emits it
///   for `S01E02`-style titles; Identify also produces it by remapping a
///   transient [`Coordinates::Absolute`] (see below). Its `absolute` field is
///   `Some(_)` only once Identify has reconciled the anime absolute number.
/// - [`Coordinates::Daily`] — a date-addressed broadcast (daily shows). The
///   parser emits it; Identify resolves it to an [`Coordinates::Episode`] via the
///   series' air-date table. **Parser-stage / transient.**
/// - [`Coordinates::SeasonPack`] — a whole-season release. The parser emits it
///   for season-pack titles; Identify fans it out to one episode node per
///   covered episode. **Parser-stage / transient.**
/// - [`Coordinates::Absolute`] — an anime absolute episode number, *before*
///   Identify uses the scene mapping to remap it to an [`Coordinates::Episode`]
///   `{ season, episode, absolute: Some(n) }`. **Parser-stage / transient.**
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
// `Ord`/`PartialOrd` are derived so coordinates can key ordered sets and maps
// (cellarr-media flags fanned-out episode nodes need a deterministic, sortable
// order — e.g. a `BTreeSet<Coordinates>` of the episodes a season pack covers).
// The derived order follows variant declaration order then field order, which is
// stable but not otherwise semantically meaningful.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Coordinates {
    /// A movie is its own unit; it carries no coordinates.
    Movie,
    /// A television episode, addressed by season and episode, optionally with the
    /// anime absolute number when known. This is the canonical TV addressing;
    /// Identify produces it from [`Coordinates::Daily`], [`Coordinates::Absolute`],
    /// and [`Coordinates::SeasonPack`] as well as from direct `SxxEyy` parses.
    Episode {
        /// Season number (specials are conventionally season 0).
        season: u32,
        /// Episode number within the season.
        episode: u32,
        /// Absolute episode number across the whole series (anime numbering),
        /// populated by Identify when it remaps a [`Coordinates::Absolute`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        absolute: Option<u32>,
    },
    /// A date-addressed broadcast (a daily show), as advertised before Identify
    /// resolves it to an [`Coordinates::Episode`]. **Parser-stage / transient.**
    Daily {
        /// The air date in ISO `yyyy-mm-dd` form. Kept as a `String` so core
        /// takes no calendar/`chrono` dependency; validated by `cellarr-parse`.
        date: String,
    },
    /// A whole-season release, before Identify fans it out to per-episode nodes.
    /// **Parser-stage / transient.**
    SeasonPack {
        /// The season the pack covers.
        season: u16,
    },
    /// An anime absolute episode number, before Identify remaps it (via the scene
    /// mapping) to an [`Coordinates::Episode`]. **Parser-stage / transient.**
    Absolute {
        /// The absolute episode number across the whole series.
        number: u32,
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
            // Episode plus the transient parser-stage TV variants Identify remaps
            // into it all address television content.
            Coordinates::Episode { .. }
            | Coordinates::Daily { .. }
            | Coordinates::SeasonPack { .. }
            | Coordinates::Absolute { .. } => MediaType::Tv,
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

/// The structural role a [`ContentNode`] plays within its media type's tree.
///
/// This is the `content.kind` discriminator from [`docs/02-data-model.md`]: it
/// names the node's level in the adjacency list (e.g. a TV `Series` has `Season`
/// children, each with `Episode` children) without the pipeline ever having to
/// branch on [`MediaType`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentKind {
    /// A film (flat; its own unit).
    Movie,
    /// A television series (root of the TV tree).
    Series,
    /// A season under a series.
    Season,
    /// An episode under a season.
    Episode,
    /// A music artist (root of the music tree).
    Artist,
    /// An album under an artist.
    Album,
    /// A track under an album.
    Track,
    /// A book author (root of the book tree).
    Author,
    /// A book under an author.
    Book,
}

/// A persisted `content` row: one node in the structural adjacency-list tree.
///
/// Where [`ContentRef`] is the slim handle the pipeline carries, `ContentNode`
/// is the full row `cellarr-db` writes and reads. The `parent_id` link is what
/// makes the tree an adjacency list (series → season → episode, artist → album →
/// track, author → book); roots have `parent_id == None`. See
/// [`docs/02-data-model.md`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentNode {
    /// This node's identifier.
    pub id: ContentId,
    /// The library the node belongs to.
    pub library_id: LibraryId,
    /// The media type (carried so callers pick the right `MediaModule`).
    pub media_type: MediaType,
    /// The parent node in the tree, or `None` for a root (series/artist/author,
    /// or a flat movie).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<ContentId>,
    /// The node's structural role within its media type.
    pub kind: ContentKind,
    /// For a TV series, how it is numbered ([`SeriesType`]) — the switch that
    /// turns on anime absolute→season/episode remapping. Carried on every node
    /// (defaulting to [`SeriesType::Standard`]) but only meaningful on a
    /// `Series`-kind TV node; the remap reads the *series root's* value by
    /// walking up the tree. Defaulted so a node decoded from a pre-`series_type`
    /// row (or a non-TV node) reads as `Standard`, preserving prior behaviour.
    #[serde(default)]
    pub series_type: SeriesType,
    /// The node's numbering within its type.
    pub coords: Coordinates,
    /// Whether the node is monitored for acquisition.
    pub monitored: bool,
    /// Link to the typed identity/metadata row, when one has been resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_id: Option<TitleId>,
    /// The tag ids associated with this node, read from the `content_tag`
    /// association. These drive tag-scoped routing: a tag-scoped delay profile,
    /// indexer, download client, or notification applies to this node only when
    /// it shares a tag. Empty (the default) means the node is untagged — only
    /// global (untagged) config applies, preserving prior behavior. Not persisted
    /// on the `content` row itself (the association is its own table); the repo
    /// populates it on read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<u32>,
}

impl ContentNode {
    /// The slim [`ContentRef`] view of this node, for handing to the pipeline.
    #[must_use]
    pub fn as_ref(&self) -> ContentRef {
        ContentRef {
            id: self.id,
            library_id: self.library_id,
            media_type: self.media_type,
            coords: self.coords.clone(),
        }
    }
}

/// The content-scoped metadata facts persisted at Identify/Refresh and read back
/// by the detail endpoints and the calendar.
///
/// This is the resolved, type-agnostic projection a node carries once a metadata
/// source has identified it: the display facts (`title`/`year`/`overview`/
/// `runtime`) and the dated facts the calendar windows on. For a movie, `air_date`
/// is the theatrical/physical release date and `digital_date` is the digital
/// (home/streaming) release; for an episode, `air_date` is the broadcast date and
/// `digital_date` is unused. Every field but the link is optional so a partial
/// identify (e.g. a title-only match) still persists a usable row.
///
/// Dates are ISO `yyyy-mm-dd` strings, kept string-comparable so the calendar's
/// `[start, end]` windowing is a plain lexical compare.
// No `Eq`: `rating` is an `f32` (PartialEq only). Equality comparisons in tests
// use `assert_eq!` (PartialEq), so this is sufficient.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContentMetadata {
    /// Display title resolved by the metadata source, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Release/first-air year, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    /// Overview/synopsis, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    /// Runtime in minutes (movie running time, or episode length), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<u32>,
    /// A movie's theatrical/physical release date, or an episode's air date, in
    /// ISO `yyyy-mm-dd` form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
    /// A movie's digital (home/streaming) release date, when distinct from the
    /// theatrical release. Unused for episodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digital_date: Option<String>,
    /// Genres resolved by the metadata source (e.g. `["Animation", "Comedy"]`),
    /// most significant first. Empty when unresolved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    /// Primary-source user rating on a 0–10 scale (TMDB `vote_average`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<f32>,
    /// Number of votes backing `rating` (TMDB `vote_count`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating_votes: Option<u32>,
}

impl ContentMetadata {
    /// Whether this record carries no facts at all (every field empty). Used to
    /// avoid persisting an empty row for a node that did not resolve anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.year.is_none()
            && self.overview.is_none()
            && self.runtime.is_none()
            && self.air_date.is_none()
            && self.digital_date.is_none()
            && self.genres.is_empty()
            && self.rating.is_none()
            && self.rating_votes.is_none()
    }
}

/// A persisted `media_file` row: a physical file on disk and its assessed
/// quality.
///
/// One file can satisfy several content nodes (a multi-episode `.mkv`); that
/// many-to-many link is modeled separately (the `content_file` table) and so is
/// not carried here. See [`docs/02-data-model.md`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaFile {
    /// File identifier.
    pub id: MediaFileId,
    /// Absolute path on disk.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// The quality assessed for this file (the same vocabulary the decision
    /// engine ranks).
    pub quality: Quality,
    /// Detected languages (ISO-639 codes or names, as resolved).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    /// Opaque media-info payload (codecs, streams, runtime) as probed by the
    /// import scanner; `None` until probed. Kept as JSON so core stays free of
    /// any probe library's schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_info: Option<serde_json::Value>,
    /// The custom-format score this file earned, when scored; `None` until the
    /// decision engine has evaluated it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_format_score: Option<i32>,
    /// The durable release type the file was imported as
    /// ([`crate::ReleaseType`]), carried from the grab. The reconcile/upgrade
    /// decision reads this back instead of re-parsing the release title, so a
    /// re-discovered full-season pack of equal standing is recognized as already
    /// held and not re-grabbed (the season-pack re-grab-loop fix). `None` for
    /// files written before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_type: Option<crate::ReleaseType>,
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
