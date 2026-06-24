//! Concrete notification (Connect) providers and their delivery seams.
//!
//! `cellarr-core` defines the [`NotificationSender`](cellarr_core::NotificationSender)
//! contract and the provider-agnostic [`NotificationMessage`](cellarr_core::NotificationMessage);
//! this module ships the live implementations that own the relevant clients
//! (HTTP via [`http::ReqwestHttpClient`], SMTP via [`email::RawSmtpTransport`],
//! and a subprocess [`script::ProcessScriptRunner`]). The pipeline dispatcher in
//! `cellarr-jobs` holds these behind the `dyn` trait, so the dispatch path stays
//! offline-testable while the wiring (here) injects the real clients.
//!
//! The set covers the common/important providers — **Discord**, **Telegram**,
//! **Email/SMTP**, **Custom Script**, the generic **Webhook**, and the media-
//! server rescan providers **Plex**, **Jellyfin**, **Emby**. Niche connectors
//! (Pushover, Slack, Gotify, Pushbullet, …) are a documented follow-up; see the
//! TODO below.

pub mod email;
pub mod http;
pub mod providers;
pub mod providers_support;
pub mod script;

use std::sync::Arc;

use cellarr_core::NotificationSender;

use self::email::{EmailSender, RawSmtpTransport};
use self::http::ReqwestHttpClient;
use self::providers::{
    DiscordSender, JellyfinEmbySender, PlexSender, TelegramSender, WebhookSender,
};
use self::script::{CustomScriptSender, ProcessScriptRunner};

// TODO: niche notification connectors. The set below is the common/important
// one the roadmap calls for. Pushover, Slack, Gotify, Pushbullet, Ntfy,
// Prowl, Join, Apprise, Mailgun/SendGrid HTTP, and the *arr "Notifiarr" relay
// are deliberately deferred — each is a thin variation on the Discord/Telegram
// HTTP-POST or the SMTP path and can be added as another `NotificationSender`
// without touching the dispatcher.

/// Build the default set of live notification senders, each wired to a real
/// client. The pipeline dispatcher routes a message to the sender whose
/// [`kind`](NotificationSender::kind) matches the notification's `kind`.
///
/// This is the one place the concrete clients are injected; everything
/// downstream sees only `dyn NotificationSender`, which keeps the dispatch path
/// offline-testable.
#[must_use]
pub fn default_senders() -> Vec<Arc<dyn NotificationSender>> {
    let http: Arc<dyn http::HttpClient> = Arc::new(ReqwestHttpClient::new());
    let smtp: Arc<dyn email::SmtpTransport> = Arc::new(RawSmtpTransport::new());
    let script: Arc<dyn script::ScriptRunner> = Arc::new(ProcessScriptRunner::new());
    vec![
        Arc::new(DiscordSender::new(Arc::clone(&http))),
        Arc::new(TelegramSender::new(Arc::clone(&http))),
        Arc::new(WebhookSender::new(Arc::clone(&http))),
        Arc::new(PlexSender::new(Arc::clone(&http))),
        Arc::new(JellyfinEmbySender::jellyfin(Arc::clone(&http))),
        Arc::new(JellyfinEmbySender::emby(Arc::clone(&http))),
        Arc::new(EmailSender::new(smtp)),
        Arc::new(CustomScriptSender::new(script)),
    ]
}
