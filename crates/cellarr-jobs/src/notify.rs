//! Connect-webhook dispatch for the pipeline.
//!
//! The runner produces a [`WebhookPayload`] at the real Grab/Import(Download)/
//! Rename transitions (and the health/test paths produce their own). This module
//! fans one payload out to every **enabled webhook notification** the user has
//! configured (`/api/v3/notification`), respecting each notification's
//! `on_events` subscription, and delivers it via the injected
//! [`WebhookSender`](cellarr_core::WebhookSender).
//!
//! A webhook failure must never break the pipeline: every send is best-effort and
//! logged; a dead receiver is a `warn!`, not a pipeline error.

use std::sync::Arc;

use cellarr_core::{NotificationConfig, WebhookPayload, WebhookSender};
use cellarr_db::Database;

/// The notification kind string a webhook notification carries (the
/// `/api/v3/notification` `implementation`/kind for the Connect webhook).
pub const WEBHOOK_KIND: &str = "webhook";

/// The settings key a webhook notification stores its target URL under (mirrors
/// the Sonarr/Radarr Webhook connector's `url` field).
pub const WEBHOOK_URL_FIELD: &str = "url";

/// Dispatches webhook payloads to the configured notifications.
///
/// Cheap to clone (an `Arc` sender + a `Database` handle). Held by the runner
/// behind an `Option` so the offline/default path sends nothing.
#[derive(Clone)]
pub struct WebhookNotifier {
    db: Database,
    sender: Arc<dyn WebhookSender>,
    /// The instance name stamped onto every payload (the app identity).
    instance_name: String,
}

impl WebhookNotifier {
    /// Build a notifier over the database (for reading notification configs) and
    /// an HTTP [`WebhookSender`].
    #[must_use]
    pub fn new(
        db: Database,
        sender: Arc<dyn WebhookSender>,
        instance_name: impl Into<String>,
    ) -> Self {
        Self {
            db,
            sender,
            instance_name: instance_name.into(),
        }
    }

    /// Fire `payload` to every enabled webhook notification subscribed to its
    /// `eventType`. Best-effort: each delivery failure is logged, never returned.
    pub async fn dispatch(&self, mut payload: WebhookPayload) {
        // Stamp the instance identity (callers leave it for the dispatcher so the
        // configured name is authoritative).
        if payload.instance_name.is_empty() {
            payload.instance_name = self.instance_name.clone();
        }
        let notifications = match self.db.config().list_notifications().await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "webhook dispatch: failed to read notifications");
                return;
            }
        };
        for n in &notifications {
            if let Some(url) = target_url(n, &payload) {
                if let Err(detail) = self.sender.send(&url, &payload).await {
                    tracing::warn!(
                        notification = %n.name,
                        event_type = payload.event_type.as_wire(),
                        detail,
                        "webhook delivery failed (continuing)"
                    );
                }
            }
        }
    }
}

/// The URL a notification should receive `payload` on, or `None` if it should not
/// (disabled, not a webhook, not subscribed to the event, or missing a URL).
fn target_url(n: &NotificationConfig, payload: &WebhookPayload) -> Option<String> {
    if !n.enabled || !n.kind.eq_ignore_ascii_case(WEBHOOK_KIND) {
        return None;
    }
    if !payload.is_enabled_by(&n.on_events) {
        return None;
    }
    n.settings
        .get(WEBHOOK_URL_FIELD)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::{MediaType, WebhookEventType, WebhookSubject};
    use serde_json::json;

    fn webhook_notification(
        url: &str,
        on_events: Vec<String>,
        enabled: bool,
    ) -> NotificationConfig {
        NotificationConfig {
            id: "n1".into(),
            name: "mock".into(),
            kind: "webhook".into(),
            enabled,
            on_events,
            settings: json!({ "url": url }),
        }
    }

    fn grab_payload() -> WebhookPayload {
        WebhookPayload::for_subject(
            WebhookEventType::Grab,
            MediaType::Movie,
            WebhookSubject {
                id: "x".into(),
                title: "M".into(),
                year: None,
                tvdb_id: None,
                tmdb_id: None,
                imdb_id: None,
            },
            "Radarr",
        )
    }

    #[test]
    fn target_url_resolves_enabled_subscribed_webhook() {
        let n = webhook_notification("http://x/y", vec![], true);
        assert_eq!(target_url(&n, &grab_payload()), Some("http://x/y".into()));
    }

    #[test]
    fn disabled_or_unsubscribed_or_nonwebhook_yields_none() {
        assert_eq!(
            target_url(&webhook_notification("u", vec![], false), &grab_payload()),
            None
        );
        assert_eq!(
            target_url(
                &webhook_notification("u", vec!["download".into()], true),
                &grab_payload()
            ),
            None
        );
        let mut discord = webhook_notification("u", vec![], true);
        discord.kind = "discord".into();
        assert_eq!(target_url(&discord, &grab_payload()), None);
    }
}
