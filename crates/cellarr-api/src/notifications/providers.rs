//! The HTTP-backed notification providers: Discord, Telegram, the generic
//! Webhook, and the media-server rescan providers (Plex, Jellyfin, Emby).
//!
//! Each implements [`NotificationSender`] over an injected [`HttpClient`] so the
//! request it builds is asserted by a record/replay test with no live service.
//! A provider reads its target from the notification's `settings` JSON, builds
//! the one [`HttpRequest`] its API expects, and maps a non-2xx response to an
//! `Err(detail)` the dispatcher logs (never a panic, never a pipeline break).

use std::sync::Arc;

use async_trait::async_trait;
use cellarr_core::notification::kind;
use cellarr_core::{NotificationConfig, NotificationMessage, NotificationSender};
use serde_json::{json, Value};

use super::http::{HttpClient, HttpRequest};
use super::providers_support::{optional_str, required_str};

/// Trim a trailing slash off a base URL so `join`s never double it.
fn trim_base(url: &str) -> &str {
    url.trim_end_matches('/')
}

/// Map an HTTP execution to the `Result<(), String>` a sender returns: a
/// transport error or a non-2xx status both become `Err(detail)`.
async fn execute_expecting_success(
    http: &dyn HttpClient,
    request: HttpRequest,
    what: &str,
) -> Result<(), String> {
    let url = request.url.clone();
    let resp = http.execute(request).await?;
    if resp.is_success() {
        Ok(())
    } else {
        Err(format!(
            "{what} to {url} returned status {}{}",
            resp.status,
            short_body(&resp.body)
        ))
    }
}

/// A short, single-line suffix of a response body for an error detail.
fn short_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        let snippet: String = trimmed.chars().take(160).collect();
        format!(": {}", snippet.replace('\n', " "))
    }
}

// --- Discord ---------------------------------------------------------------

/// Posts a rich embed to a Discord channel webhook (`settings.url`).
pub struct DiscordSender {
    http: Arc<dyn HttpClient>,
}

impl DiscordSender {
    /// Build over an HTTP client.
    #[must_use]
    pub fn new(http: Arc<dyn HttpClient>) -> Self {
        Self { http }
    }

    /// Build the Discord embed payload for a message.
    fn payload(message: &NotificationMessage) -> Value {
        let mut fields = Vec::new();
        if let Some(r) = &message.release {
            fields.push(json!({ "name": "Release", "value": r.release_title }));
            if let Some(q) = &r.quality {
                fields.push(json!({ "name": "Quality", "value": q }));
            }
        }
        if !message.files.is_empty() {
            fields.push(json!({ "name": "Files", "value": message.files.join("\n") }));
        }
        if let Some(h) = &message.health {
            fields.push(json!({ "name": "Health", "value": format!("[{}] {} ({})", h.level, h.message, h.source) }));
        }
        json!({
            "embeds": [ {
                "title": message.title(),
                "fields": fields,
            } ],
        })
    }
}

#[async_trait]
impl NotificationSender for DiscordSender {
    fn kind(&self) -> &'static str {
        kind::DISCORD
    }

    async fn send(
        &self,
        config: &NotificationConfig,
        message: &NotificationMessage,
    ) -> Result<(), String> {
        let url = required_str(config, "url")?;
        let request = HttpRequest::json_post(url, &Self::payload(message));
        execute_expecting_success(self.http.as_ref(), request, "Discord webhook").await
    }
}

// --- Telegram --------------------------------------------------------------

/// Sends a message via the Telegram bot API (`settings.botToken` + `chatId`).
pub struct TelegramSender {
    http: Arc<dyn HttpClient>,
}

impl TelegramSender {
    /// Build over an HTTP client.
    #[must_use]
    pub fn new(http: Arc<dyn HttpClient>) -> Self {
        Self { http }
    }
}

#[async_trait]
impl NotificationSender for TelegramSender {
    fn kind(&self) -> &'static str {
        kind::TELEGRAM
    }

    async fn send(
        &self,
        config: &NotificationConfig,
        message: &NotificationMessage,
    ) -> Result<(), String> {
        let token = required_str(config, "botToken")?;
        let chat_id = required_str(config, "chatId")?;
        let url = format!("https://api.telegram.org/bot{token}/sendMessage");
        let body = json!({ "chat_id": chat_id, "text": message.body() });
        let request = HttpRequest::json_post(url, &body);
        execute_expecting_success(self.http.as_ref(), request, "Telegram sendMessage").await
    }
}

// --- generic Webhook -------------------------------------------------------

/// Posts the message as a generic JSON body to an arbitrary URL
/// (`settings.url`). Unlike the Connect [`WebhookSender`](cellarr_core::WebhookSender)
/// — which posts the v3 `eventType` shape Bazarr/Notifiarr read — this posts the
/// provider-agnostic [`NotificationMessage`] for a consumer that wants cellarr's
/// own richer shape.
pub struct WebhookSender {
    http: Arc<dyn HttpClient>,
}

impl WebhookSender {
    /// Build over an HTTP client.
    #[must_use]
    pub fn new(http: Arc<dyn HttpClient>) -> Self {
        Self { http }
    }
}

#[async_trait]
impl NotificationSender for WebhookSender {
    fn kind(&self) -> &'static str {
        kind::WEBHOOK
    }

    async fn send(
        &self,
        config: &NotificationConfig,
        message: &NotificationMessage,
    ) -> Result<(), String> {
        let url = required_str(config, "url")?;
        let body = serde_json::to_value(message).map_err(|e| format!("serialize message: {e}"))?;
        let mut request = HttpRequest::json_post(url, &body);
        // Optional HTTP Basic auth, mirroring the Connect webhook's username/password.
        if let (Some(user), Some(pass)) = (
            optional_str(config, "username"),
            optional_str(config, "password"),
        ) {
            request = request.with_header("authorization", basic_auth_header(user, pass));
        }
        execute_expecting_success(self.http.as_ref(), request, "Webhook POST").await
    }
}

/// Build an HTTP Basic `Authorization` header value from credentials.
fn basic_auth_header(user: &str, pass: &str) -> String {
    use std::fmt::Write;
    // Minimal base64 (RFC 4648) of `user:pass` — avoids a base64 dep for the one
    // place the API layer needs it.
    let raw = format!("{user}:{pass}");
    let mut out = String::from("Basic ");
    let _ = write!(out, "{}", base64_encode(raw.as_bytes()));
    out
}

/// Standard base64 encoding of `input` (no line wrapping).
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

// --- Plex (media-server rescan) --------------------------------------------

/// Triggers a Plex library refresh so a freshly-imported file shows up without
/// waiting for Plex's own scan (`settings.url` + `token`). Only acts on
/// library-changing events (import/upgrade); other events are a no-op success.
pub struct PlexSender {
    http: Arc<dyn HttpClient>,
}

impl PlexSender {
    /// Build over an HTTP client.
    #[must_use]
    pub fn new(http: Arc<dyn HttpClient>) -> Self {
        Self { http }
    }
}

#[async_trait]
impl NotificationSender for PlexSender {
    fn kind(&self) -> &'static str {
        kind::PLEX
    }

    async fn send(
        &self,
        config: &NotificationConfig,
        message: &NotificationMessage,
    ) -> Result<(), String> {
        // A rescan only matters once files land; skip a grab/health/test message.
        if !message.event.changes_library() {
            return Ok(());
        }
        let base = trim_base(required_str(config, "url")?);
        let token = required_str(config, "token")?;
        // Refresh all sections (the section-scoped refresh needs a section id we
        // do not yet resolve; refreshing all is the safe, always-correct call).
        let url = format!("{base}/library/sections/all/refresh?X-Plex-Token={token}");
        let request = HttpRequest::get(url).with_header("accept", "application/json");
        execute_expecting_success(self.http.as_ref(), request, "Plex refresh").await
    }

    async fn test(&self, config: &NotificationConfig) -> Result<(), String> {
        let base = trim_base(required_str(config, "url")?);
        let token = required_str(config, "token")?;
        // `/identity` is the canonical unauthenticated-but-token-scoped liveness
        // ping; a 200 confirms the URL + token reach a Plex server.
        let url = format!("{base}/identity?X-Plex-Token={token}");
        let request = HttpRequest::get(url).with_header("accept", "application/json");
        execute_expecting_success(self.http.as_ref(), request, "Plex identity ping").await
    }
}

// --- Jellyfin / Emby (media-server rescan) ---------------------------------

/// The Jellyfin and Emby APIs are wire-compatible for the two calls cellarr
/// needs — `POST /Library/Refresh` to scan and `GET /System/Info` to ping — both
/// authenticated by an `X-Emby-Token` API-key header. This one sender backs both
/// providers; its [`kind`](NotificationSender::kind) is set at construction.
pub struct JellyfinEmbySender {
    http: Arc<dyn HttpClient>,
    kind: &'static str,
    label: &'static str,
}

impl JellyfinEmbySender {
    /// A Jellyfin sender.
    #[must_use]
    pub fn jellyfin(http: Arc<dyn HttpClient>) -> Self {
        Self {
            http,
            kind: kind::JELLYFIN,
            label: "Jellyfin",
        }
    }

    /// An Emby sender.
    #[must_use]
    pub fn emby(http: Arc<dyn HttpClient>) -> Self {
        Self {
            http,
            kind: kind::EMBY,
            label: "Emby",
        }
    }
}

#[async_trait]
impl NotificationSender for JellyfinEmbySender {
    fn kind(&self) -> &'static str {
        self.kind
    }

    async fn send(
        &self,
        config: &NotificationConfig,
        message: &NotificationMessage,
    ) -> Result<(), String> {
        if !message.event.changes_library() {
            return Ok(());
        }
        let base = trim_base(required_str(config, "url")?);
        let api_key = required_str(config, "apiKey")?;
        let request = HttpRequest::json_post(format!("{base}/Library/Refresh"), &json!({}))
            .with_header("x-emby-token", api_key);
        execute_expecting_success(
            self.http.as_ref(),
            request,
            &format!("{} library refresh", self.label),
        )
        .await
    }

    async fn test(&self, config: &NotificationConfig) -> Result<(), String> {
        let base = trim_base(required_str(config, "url")?);
        let api_key = required_str(config, "apiKey")?;
        let request =
            HttpRequest::get(format!("{base}/System/Info")).with_header("x-emby-token", api_key);
        execute_expecting_success(
            self.http.as_ref(),
            request,
            &format!("{} system info ping", self.label),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"user:pass"), "dXNlcjpwYXNz");
    }

    #[test]
    fn required_str_reports_missing_key() {
        let cfg = NotificationConfig {
            id: "1".into(),
            name: "n".into(),
            kind: "discord".into(),
            enabled: true,
            on_events: vec![],
            settings: json!({}),
        };
        let err = required_str(&cfg, "url").unwrap_err();
        assert!(err.contains("url"));
    }

    #[test]
    fn discord_payload_carries_title_and_release_fields() {
        let msg = NotificationMessage::new(cellarr_core::NotificationEvent::Grab, "cellarr")
            .with_release(cellarr_core::NotificationRelease {
                release_title: "Show.S01E01.1080p".into(),
                quality: Some("WEBDL-1080p".into()),
                indexer: None,
                size: None,
            });
        let p = DiscordSender::payload(&msg);
        assert_eq!(p["embeds"][0]["title"], "cellarr — Grabbed");
        assert_eq!(p["embeds"][0]["fields"][0]["value"], "Show.S01E01.1080p");
    }
}
