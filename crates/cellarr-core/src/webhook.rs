//! Connect-webhook payloads and the outbound-notification seam.
//!
//! The Sonarr/Radarr ecosystem (Bazarr-push, Notifiarr, generic notification
//! connectors) consumes a **Connect webhook**: an HTTP POST whose JSON body
//! carries an [`eventType`](WebhookPayload) discriminator (`Grab`, `Download`,
//! `Rename`, `Health`, `Test`, …) plus the `series`/`movie` and
//! `release`/`episodes` objects the receiver branches on. cellarr fires this from
//! the real pipeline transitions (Grab/Import/Rename) and health, and on demand
//! for the `Test` event.
//!
//! These are *values and a trait* here — `cellarr-core` does no I/O. The HTTP
//! delivery lives in a [`WebhookSender`] implementation in a crate that owns an
//! HTTP client (the API layer ships the real `reqwest` one); the pipeline runner
//! holds the seam behind a `dyn` so it stays offline-testable against a mock.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::media::MediaType;
use crate::release::Release;

/// The `eventType` discriminator the ecosystem branches on.
///
/// These match the Sonarr/Radarr Connect event names exactly (PascalCase on the
/// wire) because receivers (Bazarr, Notifiarr) string-match them. The set here is
/// the one the roadmap's Phase F exit gate requires: Grab, Download (= import),
/// Rename, Health, and Test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebhookEventType {
    /// A release was grabbed and handed to a download client.
    Grab,
    /// A completed download was imported into the library (Sonarr/Radarr name
    /// the import event `Download`, which is a long-standing wart the ecosystem
    /// keys on — kept for compatibility).
    Download,
    /// Imported files were renamed.
    Rename,
    /// A health check changed state.
    Health,
    /// A test notification, fired by `POST /notification/test`.
    Test,
}

impl WebhookEventType {
    /// The exact on-the-wire string a receiver matches on.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            WebhookEventType::Grab => "Grab",
            WebhookEventType::Download => "Download",
            WebhookEventType::Rename => "Rename",
            WebhookEventType::Health => "Health",
            WebhookEventType::Test => "Test",
        }
    }
}

/// The `series`/`movie` subject object a webhook carries.
///
/// The receiver keys on which field is present (`series` vs `movie`) to know the
/// media type, so this serializes with the field name the addressed media type
/// uses; both shapes carry the identity fields (`title`, ids) tools dereference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookSubject {
    /// The content node's id (cellarr's uuid, stringified).
    pub id: String,
    /// The human title.
    pub title: String,
    /// The release/air year, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    /// The TVDB id for series, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    /// The TMDb id for movies, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    /// The IMDb id, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
}

/// The `release` object a Grab webhook carries: what was grabbed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookRelease {
    /// The advertised release title.
    pub release_title: String,
    /// The indexer the release came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexer: Option<String>,
    /// Size in bytes, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The quality name, when assessed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
}

impl WebhookRelease {
    /// Build the release object from an indexer [`Release`] and an optional
    /// assessed quality name.
    #[must_use]
    pub fn from_release(release: &Release, quality: Option<String>) -> Self {
        Self {
            release_title: release.title.clone(),
            indexer: None,
            size: release.size,
            quality,
        }
    }
}

/// One imported/renamed file's coordinates the `episodeFile`/`movieFile` carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookFile {
    /// The on-disk path the file landed at.
    pub path: String,
    /// The previous path, for a rename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
}

/// A full Connect-webhook payload, tagged by `eventType`.
///
/// Serializes to the shape the ecosystem reads: `{ "eventType": "Grab", ... }`
/// with the subject under `series` or `movie`, plus the event-specific objects.
/// A receiver dispatches on `eventType`, then reads the fields that event carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookPayload {
    /// The discriminator (`Grab`/`Download`/`Rename`/`Health`/`Test`).
    pub event_type: WebhookEventType,
    /// The TV subject, present when the event concerns a series.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<WebhookSubject>,
    /// The movie subject, present when the event concerns a movie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub movie: Option<WebhookSubject>,
    /// The grabbed release, present on `Grab`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<WebhookRelease>,
    /// The imported/renamed files, present on `Download`/`Rename`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub episode_files: Vec<WebhookFile>,
    /// A health message, present on `Health`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<WebhookHealth>,
    /// The cellarr instance name (the addressed face's app identity).
    pub instance_name: String,
    /// The application string (`Sonarr`/`Radarr`) the subject's media type maps to.
    pub application_url: String,
}

/// The `Health` event body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookHealth {
    /// The health level (`warning`/`error`).
    pub level: String,
    /// The human-readable message.
    pub message: String,
    /// The check that produced it.
    #[serde(rename = "type")]
    pub source: String,
}

impl WebhookPayload {
    /// Build a `Test` payload (the `POST /notification/test` body). It carries no
    /// subject — receivers only assert `eventType == "Test"` to confirm wiring.
    #[must_use]
    pub fn test(instance_name: impl Into<String>) -> Self {
        Self::bare(WebhookEventType::Test, instance_name, "")
    }

    /// Build a `Health` payload.
    #[must_use]
    pub fn health(
        instance_name: impl Into<String>,
        level: impl Into<String>,
        message: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        let mut p = Self::bare(WebhookEventType::Health, instance_name, "");
        p.health = Some(WebhookHealth {
            level: level.into(),
            message: message.into(),
            source: source.into(),
        });
        p
    }

    /// A minimal payload of `event_type` carrying just identity, no subject.
    fn bare(
        event_type: WebhookEventType,
        instance_name: impl Into<String>,
        application_url: impl Into<String>,
    ) -> Self {
        Self {
            event_type,
            series: None,
            movie: None,
            release: None,
            episode_files: Vec::new(),
            health: None,
            instance_name: instance_name.into(),
            application_url: application_url.into(),
        }
    }

    /// Build a `Grab`/`Download`/`Rename` payload, placing `subject` under the
    /// field its `media_type` selects (`series` for TV, `movie` otherwise) so the
    /// receiver reads the media type from which field is present.
    #[must_use]
    pub fn for_subject(
        event_type: WebhookEventType,
        media_type: MediaType,
        subject: WebhookSubject,
        instance_name: impl Into<String>,
    ) -> Self {
        let app = match media_type {
            MediaType::Tv => "Sonarr",
            _ => "Radarr",
        };
        let mut p = Self::bare(event_type, instance_name, app);
        match media_type {
            MediaType::Tv => p.series = Some(subject),
            _ => p.movie = Some(subject),
        }
        p
    }

    /// Attach the grabbed release (builder form).
    #[must_use]
    pub fn with_release(mut self, release: WebhookRelease) -> Self {
        self.release = Some(release);
        self
    }

    /// Attach imported/renamed files (builder form).
    #[must_use]
    pub fn with_files(mut self, files: Vec<WebhookFile>) -> Self {
        self.episode_files = files;
        self
    }

    /// Whether this payload's `eventType` is one the notification config opts into
    /// via its `on_events` keys. The keys are the lowercased wire names
    /// (`grab`/`download`/`rename`/`health`/`test`); an empty `on_events` means
    /// "all events". `Test` always fires (it is an explicit, user-triggered probe).
    #[must_use]
    pub fn is_enabled_by(&self, on_events: &[String]) -> bool {
        if self.event_type == WebhookEventType::Test || on_events.is_empty() {
            return true;
        }
        let key = self.event_type.as_wire().to_ascii_lowercase();
        on_events.iter().any(|e| e.eq_ignore_ascii_case(&key))
    }
}

/// The outbound-delivery seam: send a [`WebhookPayload`] to a URL.
///
/// `cellarr-core` defines the contract; the concrete HTTP delivery (with timeout,
/// optional basic-auth header) lives in a crate that owns an HTTP client. The
/// pipeline runner holds this behind a `dyn` so the webhook path is testable with
/// a mock sender that records calls instead of hitting the network.
#[async_trait]
pub trait WebhookSender: Send + Sync {
    /// POST `payload` (as JSON) to `url`. Implementations bound the wait and
    /// surface a failure as `Err(detail)` — a webhook failure must never break the
    /// pipeline, so callers log and continue rather than propagate.
    async fn send(&self, url: &str, payload: &WebhookPayload) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grab_payload_serializes_with_event_type_and_subject() {
        let subject = WebhookSubject {
            id: "abc".into(),
            title: "The Matrix".into(),
            year: Some(1999),
            tvdb_id: None,
            tmdb_id: Some(603),
            imdb_id: Some("tt0133093".into()),
        };
        let payload = WebhookPayload::for_subject(
            WebhookEventType::Grab,
            MediaType::Movie,
            subject,
            "Radarr",
        )
        .with_release(WebhookRelease {
            release_title: "The.Matrix.1999.1080p.BluRay-GRP".into(),
            indexer: Some("fake".into()),
            size: Some(8_000_000_000),
            quality: Some("Bluray-1080p".into()),
        });
        let v = serde_json::to_value(&payload).unwrap();
        assert_eq!(v["eventType"], "Grab");
        assert_eq!(v["movie"]["title"], "The Matrix");
        assert_eq!(v["movie"]["tmdbId"], 603);
        assert!(v.get("series").is_none());
        assert_eq!(
            v["release"]["releaseTitle"],
            "The.Matrix.1999.1080p.BluRay-GRP"
        );
    }

    #[test]
    fn test_event_always_enabled_even_when_not_subscribed() {
        let p = WebhookPayload::test("Sonarr");
        assert!(p.is_enabled_by(&["grab".into()]));
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["eventType"], "Test");
    }

    #[test]
    fn on_events_filters_by_lowercased_wire_name() {
        let subject = WebhookSubject {
            id: "1".into(),
            title: "Show".into(),
            year: None,
            tvdb_id: Some(81189),
            tmdb_id: None,
            imdb_id: None,
        };
        let grab =
            WebhookPayload::for_subject(WebhookEventType::Grab, MediaType::Tv, subject, "Sonarr");
        assert!(grab.is_enabled_by(&[])); // empty = all
        assert!(grab.is_enabled_by(&["Grab".into(), "Download".into()]));
        assert!(!grab.is_enabled_by(&["download".into()]));
    }
}
