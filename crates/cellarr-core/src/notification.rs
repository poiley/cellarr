//! The notification (Connect) model: events, the provider-agnostic message, and
//! the [`NotificationSender`] seam.
//!
//! The Sonarr/Radarr ecosystem calls these **Connect** providers: a configured
//! target (Discord, Telegram, an SMTP mailbox, a media server to rescan, a custom
//! script, or a generic webhook) that fires on the pipeline's lifecycle events
//! (`onGrab`/`onDownload`(import)/`onUpgrade`/`onHealthIssue`/`onHealthRestored`).
//! This module is the part `cellarr-core` owns: the **values** (the event kind,
//! the rendered message every provider formats from) and the **trait** the
//! concrete senders implement. It does no I/O — the HTTP/SMTP/script delivery
//! lives in a crate that owns those clients (the API layer ships the live ones),
//! and the pipeline dispatcher holds the seam behind a `dyn` so the whole path is
//! offline-testable against a recording mock.
//!
//! The pre-existing [`WebhookPayload`](crate::WebhookPayload) Connect webhook is
//! one *kind* of provider; this model generalizes that seam so a Discord embed, a
//! Telegram message, an email, or a Plex library refresh all dispatch from the
//! same pipeline transitions, each respecting its own per-event toggles.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::NotificationConfig;
use crate::media::MediaType;
use crate::release::Release;

/// A lifecycle event a notification provider fires on.
///
/// These map onto the `on*` toggles the ecosystem exposes and that
/// [`NotificationConfig`] models. `Grab`/`Import`/`Upgrade` come from the
/// acquisition pipeline; `HealthIssue`/`HealthRestored` from the health monitor;
/// `Test` is the explicit, user-triggered probe `POST /notification/test` sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationEvent {
    /// A release was grabbed and handed to a download client (`onGrab`).
    Grab,
    /// A completed download was imported into the library (`onDownload`).
    Import,
    /// An import replaced an existing, lower-quality file (`onUpgrade`).
    Upgrade,
    /// A health check entered a warning/error state (`onHealthIssue`).
    HealthIssue,
    /// A previously-failing health check recovered (`onHealthRestored`).
    HealthRestored,
    /// A user-triggered test probe. Always fires, regardless of toggles.
    Test,
}

impl NotificationEvent {
    /// The stable lowercase key this event is toggled by in
    /// [`NotificationConfig::on_events`]. These are the keys the `/api/v3`
    /// notification write body maps its `on*` flags onto.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            NotificationEvent::Grab => "grab",
            // The import event is keyed "download" to match the long-standing
            // Sonarr/Radarr `onDownload` wart the ecosystem (and the existing
            // webhook `on_events`) already uses.
            NotificationEvent::Import => "download",
            NotificationEvent::Upgrade => "upgrade",
            NotificationEvent::HealthIssue | NotificationEvent::HealthRestored => "health",
            NotificationEvent::Test => "test",
        }
    }

    /// A short human label for the event, used in a rendered message title.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            NotificationEvent::Grab => "Grabbed",
            NotificationEvent::Import => "Imported",
            NotificationEvent::Upgrade => "Upgraded",
            NotificationEvent::HealthIssue => "Health Issue",
            NotificationEvent::HealthRestored => "Health Restored",
            NotificationEvent::Test => "Test",
        }
    }

    /// Whether this event is one a media-server rescan provider should act on. A
    /// rescan only makes sense once new files have landed (import/upgrade); a
    /// grab, a health event, or a test never changes the on-disk library.
    #[must_use]
    pub const fn changes_library(self) -> bool {
        matches!(self, NotificationEvent::Import | NotificationEvent::Upgrade)
    }
}

/// The subject (series/movie) a notification concerns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NotificationSubject {
    /// The content node id (cellarr's uuid, stringified).
    pub id: String,
    /// The human title.
    pub title: String,
    /// The release/air year, when known.
    pub year: Option<i32>,
    /// The media type, so a provider can label TV vs movie.
    pub media_type: Option<MediaType>,
}

/// The grabbed/imported release detail a message carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NotificationRelease {
    /// The advertised release title.
    pub release_title: String,
    /// The quality name, when assessed.
    pub quality: Option<String>,
    /// The source indexer, when known.
    pub indexer: Option<String>,
    /// Size in bytes, when reported.
    pub size: Option<u64>,
}

impl NotificationRelease {
    /// Build from an indexer [`Release`] and an optional assessed quality.
    #[must_use]
    pub fn from_release(release: &Release, quality: Option<String>) -> Self {
        Self {
            release_title: release.title.clone(),
            quality,
            indexer: None,
            size: release.size,
        }
    }
}

/// A health event detail a message carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NotificationHealth {
    /// The health level (`warning`/`error`, or `ok` on restore).
    pub level: String,
    /// The human-readable message.
    pub message: String,
    /// The check that produced it.
    pub source: String,
}

/// A provider-agnostic, fully-rendered notification.
///
/// The pipeline builds one of these per event and hands it to the dispatcher,
/// which fans it to every enabled provider subscribed to its [`event`](Self::event).
/// Each [`NotificationSender`] formats it into its own wire shape (a Discord
/// embed, a Telegram message, an email body, a media-server rescan call, a
/// script's environment, or a generic JSON POST). Every field a provider might
/// render is present so a sender never needs to reach back into the database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationMessage {
    /// Which lifecycle event this message is for.
    pub event: NotificationEvent,
    /// The cellarr instance name (the app identity stamped on every message).
    pub instance_name: String,
    /// The subject (series/movie), absent for `Test`/`Health` events.
    pub subject: Option<NotificationSubject>,
    /// The grabbed/imported release, present on `Grab`/`Import`/`Upgrade`.
    pub release: Option<NotificationRelease>,
    /// The imported/renamed file paths, present on `Import`/`Upgrade`.
    pub files: Vec<String>,
    /// The health detail, present on `HealthIssue`/`HealthRestored`.
    pub health: Option<NotificationHealth>,
}

impl NotificationMessage {
    /// A bare message of `event` carrying just the instance identity.
    #[must_use]
    pub fn new(event: NotificationEvent, instance_name: impl Into<String>) -> Self {
        Self {
            event,
            instance_name: instance_name.into(),
            subject: None,
            release: None,
            files: Vec::new(),
            health: None,
        }
    }

    /// A `Test` probe message (the body `POST /notification/test` renders).
    #[must_use]
    pub fn test(instance_name: impl Into<String>) -> Self {
        Self::new(NotificationEvent::Test, instance_name)
    }

    /// Attach the subject (builder form).
    #[must_use]
    pub fn with_subject(mut self, subject: NotificationSubject) -> Self {
        self.subject = Some(subject);
        self
    }

    /// Attach the release detail (builder form).
    #[must_use]
    pub fn with_release(mut self, release: NotificationRelease) -> Self {
        self.release = Some(release);
        self
    }

    /// Attach imported/renamed files (builder form).
    #[must_use]
    pub fn with_files(mut self, files: Vec<String>) -> Self {
        self.files = files;
        self
    }

    /// Attach the health detail (builder form).
    #[must_use]
    pub fn with_health(mut self, health: NotificationHealth) -> Self {
        self.health = Some(health);
        self
    }

    /// A short one-line title every text provider (Discord/Telegram/email
    /// subject) can lead with: `"<instance> — <Label>: <subject>"`.
    #[must_use]
    pub fn title(&self) -> String {
        let label = self.event.label();
        match &self.subject {
            Some(s) if !s.title.is_empty() => match s.year {
                Some(y) => format!("{} — {label}: {} ({y})", self.instance_name, s.title),
                None => format!("{} — {label}: {}", self.instance_name, s.title),
            },
            _ => format!("{} — {label}", self.instance_name),
        }
    }

    /// A multi-line plain-text body the text providers render. Lists the release,
    /// quality, files, or health detail the event carries; never panics and
    /// always returns at least the title line.
    #[must_use]
    pub fn body(&self) -> String {
        let mut lines = vec![self.title()];
        if let Some(r) = &self.release {
            lines.push(format!("Release: {}", r.release_title));
            if let Some(q) = &r.quality {
                lines.push(format!("Quality: {q}"));
            }
            if let Some(ix) = &r.indexer {
                lines.push(format!("Indexer: {ix}"));
            }
        }
        for f in &self.files {
            lines.push(format!("File: {f}"));
        }
        if let Some(h) = &self.health {
            lines.push(format!("[{}] {} ({})", h.level, h.message, h.source));
        }
        lines.join("\n")
    }

    /// Whether a notification subscribed to `on_events` should receive this
    /// message. An empty `on_events` means "all events"; a `Test` always fires
    /// (it is an explicit, user-triggered probe). The match is case-insensitive
    /// against the event's [`key`](NotificationEvent::key).
    #[must_use]
    pub fn is_enabled_by(&self, on_events: &[String]) -> bool {
        if self.event == NotificationEvent::Test || on_events.is_empty() {
            return true;
        }
        let key = self.event.key();
        on_events.iter().any(|e| e.eq_ignore_ascii_case(key))
    }
}

/// The Connect-provider implementation keys cellarr ships, as stored in
/// [`NotificationConfig::kind`]. These select which [`NotificationSender`]
/// formats a [`NotificationMessage`] and map to the v3 `implementation` strings
/// the ecosystem round-trips.
pub mod kind {
    /// A Discord channel webhook (posts an embed).
    pub const DISCORD: &str = "discord";
    /// A Telegram bot (sends a message to a chat).
    pub const TELEGRAM: &str = "telegram";
    /// An SMTP mailbox (sends an email).
    pub const EMAIL: &str = "email";
    /// A local executable run with the event in its environment.
    pub const CUSTOM_SCRIPT: &str = "customscript";
    /// A generic JSON webhook (the pre-existing Connect push).
    pub const WEBHOOK: &str = "webhook";
    /// A Plex Media Server library refresh.
    pub const PLEX: &str = "plex";
    /// A Jellyfin library scan.
    pub const JELLYFIN: &str = "jellyfin";
    /// An Emby library scan.
    pub const EMBY: &str = "emby";
}

/// Whether `config` should receive `message`: it must be enabled and subscribed
/// to the message's event. A shared gate the dispatcher applies before handing a
/// message to any sender, so the toggle semantics live in exactly one place.
#[must_use]
pub fn config_accepts(config: &NotificationConfig, message: &NotificationMessage) -> bool {
    config.enabled && message.is_enabled_by(&config.on_events)
}

/// The outbound-delivery seam for a notification provider.
///
/// `cellarr-core` defines the contract; each concrete sender (Discord, Telegram,
/// SMTP, custom script, generic webhook, and the media-server rescan providers)
/// lives in a crate that owns the relevant client. The pipeline dispatcher holds
/// senders behind a `dyn` keyed by [`kind`](NotificationConfig::kind) so the path
/// is testable with recording mocks instead of live services.
///
/// A sender is best-effort by contract: it returns `Err(detail)` on failure, and
/// the dispatcher logs and continues — a failing provider must never break the
/// pipeline.
#[async_trait]
pub trait NotificationSender: Send + Sync {
    /// The [`NotificationConfig::kind`] this sender handles (e.g. `"discord"`).
    /// The dispatcher routes a message to the sender whose `kind` matches the
    /// notification's.
    fn kind(&self) -> &'static str;

    /// Deliver `message` to the target described by `config.settings`. Bounds its
    /// own wait and surfaces a failure as `Err(detail)`; never panics on a
    /// malformed config (a missing required setting is an `Err`, not a panic).
    async fn send(
        &self,
        config: &NotificationConfig,
        message: &NotificationMessage,
    ) -> Result<(), String>;

    /// Probe the target's connectivity for `POST /notification/test`. The default
    /// sends a [`NotificationMessage::test`] through [`send`](Self::send); a
    /// provider with a cheaper liveness check (e.g. a media-server `/identity`
    /// ping) overrides this.
    async fn test(&self, config: &NotificationConfig) -> Result<(), String> {
        let message = NotificationMessage::test(config.name.clone());
        self.send(config, &message).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_event_is_keyed_download_for_ecosystem_compat() {
        assert_eq!(NotificationEvent::Import.key(), "download");
        assert_eq!(NotificationEvent::Grab.key(), "grab");
        assert_eq!(NotificationEvent::HealthRestored.key(), "health");
    }

    #[test]
    fn only_import_and_upgrade_change_the_library() {
        assert!(NotificationEvent::Import.changes_library());
        assert!(NotificationEvent::Upgrade.changes_library());
        assert!(!NotificationEvent::Grab.changes_library());
        assert!(!NotificationEvent::HealthIssue.changes_library());
        assert!(!NotificationEvent::Test.changes_library());
    }

    #[test]
    fn test_event_always_enabled_even_when_not_subscribed() {
        let m = NotificationMessage::test("cellarr");
        assert!(m.is_enabled_by(&["grab".into()]));
    }

    #[test]
    fn empty_on_events_means_all() {
        let m = NotificationMessage::new(NotificationEvent::Grab, "cellarr");
        assert!(m.is_enabled_by(&[]));
    }

    #[test]
    fn on_events_filters_by_event_key() {
        let import = NotificationMessage::new(NotificationEvent::Import, "cellarr");
        assert!(import.is_enabled_by(&["download".into()]));
        assert!(!import.is_enabled_by(&["grab".into()]));
        let upgrade = NotificationMessage::new(NotificationEvent::Upgrade, "cellarr");
        assert!(upgrade.is_enabled_by(&["upgrade".into()]));
        assert!(!upgrade.is_enabled_by(&["download".into()]));
    }

    #[test]
    fn title_and_body_render_subject_and_release() {
        let m = NotificationMessage::new(NotificationEvent::Import, "cellarr")
            .with_subject(NotificationSubject {
                id: "1".into(),
                title: "The Matrix".into(),
                year: Some(1999),
                media_type: Some(MediaType::Movie),
            })
            .with_release(NotificationRelease {
                release_title: "The.Matrix.1999.1080p.BluRay-GRP".into(),
                quality: Some("Bluray-1080p".into()),
                indexer: None,
                size: Some(8_000_000_000),
            })
            .with_files(vec!["/movies/The Matrix (1999)/The Matrix.mkv".into()]);
        assert_eq!(m.title(), "cellarr — Imported: The Matrix (1999)");
        let body = m.body();
        assert!(body.contains("Release: The.Matrix.1999.1080p.BluRay-GRP"));
        assert!(body.contains("Quality: Bluray-1080p"));
        assert!(body.contains("File: /movies/The Matrix (1999)/The Matrix.mkv"));
    }

    #[test]
    fn config_accepts_requires_enabled_and_subscribed() {
        let cfg = NotificationConfig {
            id: "n1".into(),
            name: "d".into(),
            kind: "discord".into(),
            enabled: true,
            on_events: vec!["grab".into()],
            tags: Vec::new(),
            settings: serde_json::json!({}),
        };
        let grab = NotificationMessage::new(NotificationEvent::Grab, "cellarr");
        let import = NotificationMessage::new(NotificationEvent::Import, "cellarr");
        assert!(config_accepts(&cfg, &grab));
        assert!(!config_accepts(&cfg, &import));
        let disabled = NotificationConfig {
            enabled: false,
            ..cfg
        };
        assert!(!config_accepts(&disabled, &grab));
    }
}
