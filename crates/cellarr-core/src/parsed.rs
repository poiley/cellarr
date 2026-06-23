//! The parsed-release model with per-field confidence.
//!
//! [`ParsedRelease`] is the structured output of `cellarr-parse`. Each field is
//! carried alongside a [`Confidence`] so downstream stages (Identify, Decide,
//! and the optional inference fallback) can reason about how much to trust each
//! extracted fact rather than treating the parse as all-or-nothing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::media::Coordinates;

/// A confidence score in the inclusive range `0.0..=1.0`.
///
/// The constructor clamps out-of-range inputs so a confidence value can never
/// be nonsensical; this keeps the parser panic-free on adversarial inputs.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Confidence(f32);

impl Confidence {
    /// Maximum confidence (a deterministic, unambiguous extraction).
    pub const CERTAIN: Confidence = Confidence(1.0);
    /// Minimum confidence (no signal).
    pub const NONE: Confidence = Confidence(0.0);

    /// Construct a confidence, clamping to `0.0..=1.0`.
    #[must_use]
    pub fn new(value: f32) -> Self {
        Self(value.clamp(0.0, 1.0))
    }

    /// The raw scalar value.
    #[must_use]
    pub fn value(self) -> f32 {
        self.0
    }
}

/// The fields the parser can extract, used as keys in the confidence map.
///
/// Keeping the set closed (an enum) means a new field cannot silently appear in
/// the confidence map without being named here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParsedField {
    /// The video resolution.
    Resolution,
    /// The source/medium.
    Source,
    /// The video codec.
    Codec,
    /// The audio format.
    Audio,
    /// The HDR flags.
    Hdr,
    /// The edition.
    Edition,
    /// The language list.
    Languages,
    /// The release group.
    Group,
    /// The proper/repack flag.
    ProperRepack,
    /// The year.
    Year,
    /// The numbering / coordinates.
    Coordinates,
}

/// Video resolution buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Resolution {
    /// 480p / SD.
    R480p,
    /// 576p.
    R576p,
    /// 720p.
    R720p,
    /// 1080p.
    R1080p,
    /// 2160p / 4K.
    R2160p,
}

/// The medium a release was sourced from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// Workprint — an unfinished pre-release cut (lowest movie tier).
    Workprint,
    /// Cam — a recording taken in a cinema.
    Cam,
    /// Telesync — a cam synced to an external audio source.
    Telesync,
    /// Telecine — a film-reel-to-digital transfer.
    Telecine,
    /// Regional — an early regional retail/screener disc.
    Regional,
    /// DVD screener — a promotional DVD screener.
    Dvdscr,
    /// Standard-definition TV.
    Sdtv,
    /// High-definition TV broadcast.
    Hdtv,
    /// Raw-HD — an untouched HD broadcast/transport stream (no re-encode).
    RawHd,
    /// Web-rip (re-encoded web capture).
    Webrip,
    /// Web-DL (direct web download).
    WebDl,
    /// DVD.
    Dvd,
    /// DVD-R — a recordable-DVD copy (DVD tier, distinct bucket).
    DvdR,
    /// Blu-ray (encoded).
    Bluray,
    /// BR-DISK — a full untouched Blu-ray/UHD disc (BDMV/ISO, no encode).
    BrDisk,
    /// Remux (untouched disc stream).
    Remux,
}

/// The video codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    /// H.264 / AVC.
    X264,
    /// H.265 / HEVC.
    X265,
    /// AV1.
    Av1,
    /// MPEG-2 and similar legacy codecs.
    Other,
}

/// HDR format flags. A release may advertise several.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HdrFormat {
    /// HDR10.
    Hdr10,
    /// HDR10+.
    Hdr10Plus,
    /// Dolby Vision.
    DolbyVision,
    /// Hybrid Log-Gamma.
    Hlg,
}

/// Whether a release re-issues a flawed earlier one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProperRepack {
    /// A re-release of a flawed earlier release.
    Proper,
    /// A re-pack (re-uploaded fix), conceptually similar to proper.
    Repack,
}

/// The structured facts extracted from a release title.
///
/// Every optional field is `None` when the parser found no evidence for it.
/// The companion `confidence` map records how strongly each populated field is
/// believed. Absence from the map is equivalent to [`Confidence::NONE`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedRelease {
    /// The raw title the parse came from, retained for logging and the corpus.
    pub raw_title: String,
    /// The cleaned title with quality/numbering tokens removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean_title: Option<String>,
    /// Extracted resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
    /// Extracted source/medium.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    /// Extracted video codec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<VideoCodec>,
    /// Extracted audio descriptors (free-form; e.g. "TrueHD Atmos 7.1").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio: Vec<String>,
    /// Extracted HDR flags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hdr: Vec<HdrFormat>,
    /// Extracted edition (e.g. "Director's Cut").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edition: Option<String>,
    /// Extracted languages (ISO-639 codes or names, as found).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    /// Extracted release group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Proper/repack marker, if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proper_repack: Option<ProperRepack>,
    /// Extracted year.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    /// Extracted numbering. A single title may address several units (multi-ep),
    /// hence a vector.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coordinates: Vec<Coordinates>,
    /// Per-field confidence. Missing keys mean [`Confidence::NONE`].
    #[serde(default)]
    pub confidence: BTreeMap<ParsedField, Confidence>,
}

impl ParsedRelease {
    /// An empty parse of `raw_title`, with no fields populated.
    #[must_use]
    pub fn new(raw_title: impl Into<String>) -> Self {
        Self {
            raw_title: raw_title.into(),
            clean_title: None,
            resolution: None,
            source: None,
            codec: None,
            audio: Vec::new(),
            hdr: Vec::new(),
            edition: None,
            languages: Vec::new(),
            group: None,
            proper_repack: None,
            year: None,
            coordinates: Vec::new(),
            confidence: BTreeMap::new(),
        }
    }

    /// The confidence recorded for `field`, defaulting to [`Confidence::NONE`].
    #[must_use]
    pub fn confidence_of(&self, field: ParsedField) -> Confidence {
        self.confidence
            .get(&field)
            .copied()
            .unwrap_or(Confidence::NONE)
    }

    /// Record `confidence` for `field`.
    pub fn set_confidence(&mut self, field: ParsedField, confidence: Confidence) {
        self.confidence.insert(field, confidence);
    }

    /// The mean confidence over the fields that have a recorded value, or
    /// [`Confidence::NONE`] when nothing has been extracted. Used by the parser
    /// to decide whether to consult the inference fallback.
    #[must_use]
    pub fn aggregate_confidence(&self) -> Confidence {
        if self.confidence.is_empty() {
            return Confidence::NONE;
        }
        let sum: f32 = self.confidence.values().map(|c| c.value()).sum();
        Confidence::new(sum / self.confidence.len() as f32)
    }
}
