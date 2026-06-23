//! cellarr-media — the per-media-type modules behind [`cellarr_core::MediaModule`].
//!
//! This crate makes "one app, all media types" real: each media type is a
//! module, not a fork. The pipeline never branches on [`cellarr_core::MediaType`]
//! — it asks the [`registry::MediaRegistry`] for the matching module and
//! delegates. See `docs/specs/cellarr-media.md` and `docs/02-data-model.md`.
//!
//! # What lives here
//!
//! - [`module::MovieModule`] / [`module::TvModule`] — the v1 [`cellarr_core::MediaModule`]
//!   implementations: search terms, release matching (with confidence), and
//!   naming tokens.
//! - [`registry::MediaRegistry`] — the `MediaType` → module map.
//! - [`identify`] — Identify-side coordinate normalization, including the anime
//!   [`cellarr_core::Coordinates::Absolute`] → [`cellarr_core::Coordinates::Episode`]
//!   remap via a [`identify::SceneMappingProvider`].
//! - The seams the modules read through ([`content::ContentLookup`],
//!   [`meta::MetadataLookup`]) so the modules stay pure and mockable.
//!
//! # Library safety
//!
//! Matching never force-fits. An ambiguous title (two+ rival nodes) is returned
//! at a deliberately low confidence so the caller routes it to manual
//! resolution, and an anime absolute number the scene mapping cannot place is an
//! error, not a guess. These encode the project's never-corrupt-the-library
//! rule at the type level.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod content;
pub mod error;
pub mod identify;
pub mod meta;
pub mod module;
pub mod registry;

pub use content::{ContentCandidate, ContentLookup};
pub use error::MediaError;
pub use identify::{
    remap_absolute, remap_absolute_dyn, DynSceneMappingProvider, IdentifyError, SceneBoxError,
    SceneBoxErrorWrapper, SceneMapping, SceneMappingProvider, SceneRange,
};
pub use meta::{MetadataLookup, MovieMeta, SeriesMeta};
pub use module::{ModuleError, MovieModule, TvModule, AMBIGUOUS_CONFIDENCE};
pub use registry::{BoxError, DynMediaModule, MediaRegistry};
