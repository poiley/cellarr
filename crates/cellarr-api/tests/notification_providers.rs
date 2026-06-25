//! Record/replay tests for the concrete notification providers.
//!
//! HERMETIC: no live services.
//!  - The HTTP providers (Discord, Telegram, the generic Webhook, Plex, Jellyfin,
//!    Emby) post through a recording mock [`HttpClient`] that captures the exact
//!    request (method, URL, headers, body) and returns a canned response — so the
//!    request each provider builds is asserted without any network.
//!  - The Email provider is tested both against a recording mock [`SmtpTransport`]
//!    (the envelope mapping) and against an in-process raw-SMTP server (the real
//!    dialog the shipped [`RawSmtpTransport`] speaks).
//!  - The Custom Script provider runs a real temporary script and reads back the
//!    environment cellarr passed it.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cellarr_api::notifications::email::{Email, EmailSender, RawSmtpTransport, SmtpTransport};
use cellarr_api::notifications::http::{HttpClient, HttpMethod, HttpRequest, HttpResponse};
use cellarr_api::notifications::providers::{
    DiscordSender, JellyfinEmbySender, PlexSender, TelegramSender, WebhookSender,
};
use cellarr_api::notifications::script::{CustomScriptSender, ProcessScriptRunner};
use cellarr_core::{
    NotificationConfig, NotificationEvent, NotificationMessage, NotificationRelease,
    NotificationSender, NotificationSubject,
};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// A recording mock HTTP client.
// ---------------------------------------------------------------------------

struct RecordingHttp {
    status: u16,
    body: String,
    seen: Arc<Mutex<Vec<HttpRequest>>>,
}

impl RecordingHttp {
    fn ok() -> (Arc<Self>, Arc<Mutex<Vec<HttpRequest>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Arc::new(Self {
                status: 204,
                body: String::new(),
                seen: Arc::clone(&seen),
            }),
            seen,
        )
    }

    fn failing(status: u16, body: &str) -> Arc<Self> {
        Arc::new(Self {
            status,
            body: body.to_string(),
            seen: Arc::new(Mutex::new(Vec::new())),
        })
    }
}

#[async_trait]
impl HttpClient for RecordingHttp {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, String> {
        self.seen.lock().unwrap().push(request);
        Ok(HttpResponse {
            status: self.status,
            body: self.body.clone(),
        })
    }
}

fn config(kind: &str, settings: Value) -> NotificationConfig {
    NotificationConfig {
        tags: Vec::new(),
        id: "n1".into(),
        name: "test".into(),
        kind: kind.into(),
        enabled: true,
        on_events: vec![],
        settings,
    }
}

fn import_message() -> NotificationMessage {
    NotificationMessage::new(NotificationEvent::Import, "cellarr")
        .with_subject(NotificationSubject {
            id: "abc".into(),
            title: "The Matrix".into(),
            year: Some(1999),
            media_type: Some(cellarr_core::MediaType::Movie),
        })
        .with_release(NotificationRelease {
            release_title: "The.Matrix.1999.1080p.BluRay-GRP".into(),
            quality: Some("Bluray-1080p".into()),
            indexer: None,
            size: None,
        })
        .with_files(vec!["/movies/The Matrix (1999)/The Matrix.mkv".into()])
}

fn grab_message() -> NotificationMessage {
    NotificationMessage::new(NotificationEvent::Grab, "cellarr").with_release(NotificationRelease {
        release_title: "Show.S01E01.1080p.WEB-DL".into(),
        quality: Some("WEBDL-1080p".into()),
        indexer: None,
        size: None,
    })
}

// ---------------------------------------------------------------------------
// Discord
// ---------------------------------------------------------------------------

#[tokio::test]
async fn discord_posts_embed_to_webhook_url() {
    let (http, seen) = RecordingHttp::ok();
    let sender = DiscordSender::new(http);
    let cfg = config(
        "discord",
        json!({ "url": "https://discord.test/webhook/abc" }),
    );
    sender.send(&cfg, &grab_message()).await.unwrap();

    let reqs = seen.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, HttpMethod::Post);
    assert_eq!(reqs[0].url, "https://discord.test/webhook/abc");
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(body["embeds"][0]["title"], "cellarr — Grabbed");
    assert_eq!(
        body["embeds"][0]["fields"][0]["value"],
        "Show.S01E01.1080p.WEB-DL"
    );
}

#[tokio::test]
async fn discord_missing_url_is_an_error_not_a_panic() {
    let (http, _) = RecordingHttp::ok();
    let sender = DiscordSender::new(http);
    let cfg = config("discord", json!({}));
    let err = sender.send(&cfg, &grab_message()).await.unwrap_err();
    assert!(err.contains("url"));
}

#[tokio::test]
async fn discord_non_2xx_is_an_error() {
    let http = RecordingHttp::failing(400, "bad webhook");
    let sender = DiscordSender::new(http);
    let cfg = config("discord", json!({ "url": "https://discord.test/x" }));
    let err = sender.send(&cfg, &grab_message()).await.unwrap_err();
    assert!(err.contains("400"), "{err}");
}

// ---------------------------------------------------------------------------
// Telegram
// ---------------------------------------------------------------------------

#[tokio::test]
async fn telegram_posts_sendmessage_with_token_and_chat() {
    let (http, seen) = RecordingHttp::ok();
    let sender = TelegramSender::new(http);
    let cfg = config(
        "telegram",
        json!({ "botToken": "123:ABC", "chatId": "-1001" }),
    );
    sender.send(&cfg, &grab_message()).await.unwrap();
    let reqs = seen.lock().unwrap();
    assert_eq!(
        reqs[0].url,
        "https://api.telegram.org/bot123:ABC/sendMessage"
    );
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(body["chat_id"], "-1001");
    assert!(body["text"].as_str().unwrap().contains("Grabbed"));
}

// ---------------------------------------------------------------------------
// Generic Webhook
// ---------------------------------------------------------------------------

#[tokio::test]
async fn webhook_posts_full_message_with_optional_basic_auth() {
    let (http, seen) = RecordingHttp::ok();
    let sender = WebhookSender::new(http);
    let cfg = config(
        "webhook",
        json!({ "url": "https://hook.test/in", "username": "user", "password": "pass" }),
    );
    sender.send(&cfg, &import_message()).await.unwrap();
    let reqs = seen.lock().unwrap();
    assert_eq!(reqs[0].url, "https://hook.test/in");
    // dXNlcjpwYXNz == base64("user:pass")
    assert_eq!(
        reqs[0].headers.get("authorization").unwrap(),
        "Basic dXNlcjpwYXNz"
    );
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(body["event"], "Import");
    assert_eq!(body["subject"]["title"], "The Matrix");
}

// ---------------------------------------------------------------------------
// Plex / Jellyfin / Emby (media-server rescan)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plex_refreshes_on_import_and_pings_on_test() {
    let (http, seen) = RecordingHttp::ok();
    let sender = PlexSender::new(Arc::clone(&http) as Arc<dyn HttpClient>);
    let cfg = config(
        "plex",
        json!({ "url": "http://plex.test:32400/", "token": "tok" }),
    );
    // Import triggers a section refresh.
    sender.send(&cfg, &import_message()).await.unwrap();
    // A grab does NOT (a rescan only matters once files land).
    sender.send(&cfg, &grab_message()).await.unwrap();
    // Test pings /identity.
    sender.test(&cfg).await.unwrap();

    let reqs = seen.lock().unwrap();
    assert_eq!(reqs.len(), 2, "grab must not trigger a refresh");
    assert_eq!(reqs[0].method, HttpMethod::Get);
    assert_eq!(
        reqs[0].url,
        "http://plex.test:32400/library/sections/all/refresh?X-Plex-Token=tok"
    );
    assert!(reqs[1].url.contains("/identity?X-Plex-Token=tok"));
}

#[tokio::test]
async fn jellyfin_and_emby_refresh_with_token_header() {
    for (build, kind, label) in [
        (
            JellyfinEmbySender::jellyfin as fn(Arc<dyn HttpClient>) -> JellyfinEmbySender,
            "jellyfin",
            "Jellyfin",
        ),
        (JellyfinEmbySender::emby, "emby", "Emby"),
    ] {
        let (http, seen) = RecordingHttp::ok();
        let sender = build(http);
        assert_eq!(sender.kind(), kind);
        let cfg = config(
            kind,
            json!({ "url": "http://media.test:8096", "apiKey": "key123" }),
        );
        sender.send(&cfg, &import_message()).await.unwrap();
        sender.test(&cfg).await.unwrap();
        let reqs = seen.lock().unwrap();
        assert_eq!(
            reqs[0].url, "http://media.test:8096/Library/Refresh",
            "{label}"
        );
        assert_eq!(reqs[0].method, HttpMethod::Post);
        assert_eq!(reqs[0].headers.get("x-emby-token").unwrap(), "key123");
        assert_eq!(reqs[1].url, "http://media.test:8096/System/Info");
    }
}

// ---------------------------------------------------------------------------
// Custom Script (real temp script)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn custom_script_runs_with_event_env() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let script = dir.path().join("notify.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             printf '%s\\n%s\\n%s\\n' \
             \"$cellarr_eventtype\" \"$cellarr_movie_title\" \"$cellarr_imported_files\" > '{}'\n",
            out.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let sender = CustomScriptSender::new(Arc::new(ProcessScriptRunner::new()));
    let cfg = config("customscript", json!({ "path": script.to_string_lossy() }));
    sender.send(&cfg, &import_message()).await.unwrap();

    let captured = std::fs::read_to_string(&out).unwrap();
    let mut lines = captured.lines();
    assert_eq!(lines.next().unwrap(), "download");
    assert_eq!(lines.next().unwrap(), "The Matrix");
    assert!(lines.next().unwrap().ends_with("The Matrix.mkv"));
}

#[tokio::test]
async fn custom_script_missing_path_is_an_error() {
    let sender = CustomScriptSender::new(Arc::new(ProcessScriptRunner::new()));
    let cfg = config("customscript", json!({ "path": "/no/such/script-xyz" }));
    let err = sender.send(&cfg, &grab_message()).await.unwrap_err();
    assert!(err.contains("not found"), "{err}");
}

// ---------------------------------------------------------------------------
// Email (mock transport + a real in-process SMTP server)
// ---------------------------------------------------------------------------

struct RecordingSmtp {
    sent: Arc<Mutex<Vec<Email>>>,
}

#[async_trait]
impl SmtpTransport for RecordingSmtp {
    async fn send(&self, email: &Email) -> Result<(), String> {
        self.sent.lock().unwrap().push(email.clone());
        Ok(())
    }
}

#[tokio::test]
async fn email_builds_envelope_through_transport() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let transport = Arc::new(RecordingSmtp {
        sent: Arc::clone(&sent),
    });
    let sender = EmailSender::new(transport);
    let cfg = config(
        "email",
        json!({
            "host": "smtp.test",
            "port": 2525,
            "from": "cellarr@test",
            "to": "alice@test, bob@test",
        }),
    );
    sender.send(&cfg, &import_message()).await.unwrap();
    let mail = sent.lock().unwrap();
    assert_eq!(mail[0].host, "smtp.test");
    assert_eq!(mail[0].port, 2525);
    assert_eq!(
        mail[0].to,
        vec!["alice@test".to_string(), "bob@test".to_string()]
    );
    assert_eq!(mail[0].subject, "cellarr — Imported: The Matrix (1999)");
    assert!(mail[0]
        .render_message()
        .contains("Content-Type: text/plain"));
}

#[tokio::test]
async fn email_tls_required_is_refused_by_raw_transport() {
    let transport = RawSmtpTransport::new();
    let email = Email {
        host: "smtp.test".into(),
        port: 465,
        require_tls: true,
        auth: None,
        from: "a@test".into(),
        to: vec!["b@test".into()],
        subject: "s".into(),
        body: "b".into(),
    };
    let err = transport.send(&email).await.unwrap_err();
    assert!(err.to_lowercase().contains("tls"), "{err}");
}

#[tokio::test]
async fn raw_smtp_transport_completes_the_dialog_against_a_mock_server() {
    // A minimal in-process SMTP server that walks the submission dialog and
    // records the DATA payload, so the real RawSmtpTransport dialog is exercised
    // end to end with no external mail server.
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = Arc::clone(&captured);

    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let (read, mut write) = socket.into_split();
        let mut reader = BufReader::new(read);
        write.write_all(b"220 mock ESMTP\r\n").await.unwrap();
        let mut commands = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            let upper = line.to_ascii_uppercase();
            commands.push(line.trim_end().to_string());
            if upper.starts_with("EHLO") {
                write.write_all(b"250-mock\r\n250 OK\r\n").await.unwrap();
            } else if upper.starts_with("MAIL FROM") || upper.starts_with("RCPT TO") {
                write.write_all(b"250 OK\r\n").await.unwrap();
            } else if upper.starts_with("DATA") {
                write
                    .write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                    .await
                    .unwrap();
                // Read the message body until the lone "." terminator line.
                let mut data = String::new();
                loop {
                    let mut dl = String::new();
                    if reader.read_line(&mut dl).await.unwrap() == 0 {
                        break;
                    }
                    if dl == ".\r\n" || dl == ".\n" {
                        break;
                    }
                    data.push_str(&dl);
                }
                cap.lock().unwrap().push(data);
                write.write_all(b"250 OK queued\r\n").await.unwrap();
            } else if upper.starts_with("QUIT") {
                write.write_all(b"221 Bye\r\n").await.unwrap();
                break;
            } else {
                write.write_all(b"250 OK\r\n").await.unwrap();
            }
        }
        // Drain anything trailing so the client's QUIT write never errors.
        let mut sink = [0u8; 16];
        let _ = reader.read(&mut sink).await;
        commands
    });

    let transport = RawSmtpTransport::new();
    let email = Email {
        host: "127.0.0.1".into(),
        port,
        require_tls: false,
        auth: None,
        from: "cellarr@test".into(),
        to: vec!["alice@test".into()],
        subject: "cellarr — Imported".into(),
        body: "Imported a file".into(),
    };
    transport.send(&email).await.unwrap();
    let _ = server.await.unwrap();

    let data = captured.lock().unwrap();
    assert_eq!(data.len(), 1, "exactly one message should be delivered");
    assert!(data[0].contains("Subject: cellarr — Imported"));
    assert!(data[0].contains("Imported a file"));
}
