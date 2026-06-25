//! The Email / SMTP notification provider.
//!
//! Sends a plain-text email on each subscribed event via an SMTP submission
//! server (`settings.host`/`port`/`from`/`to`, optional `username`/`password`).
//! Delivery goes through an [`SmtpTransport`] seam so the provider is asserted by
//! a test against a recording mock (and an in-process raw-SMTP server), never a
//! live mail server in CI.
//!
//! The shipped [`RawSmtpTransport`] speaks enough of SMTP submission to deliver
//! through a typical local/relay server on the plaintext or implicit-trust path:
//! `EHLO` → optional `AUTH LOGIN` → `MAIL FROM`/`RCPT TO`/`DATA`. Implicit-TLS
//! (port 465) and STARTTLS upgrades are a documented follow-up — see the TODO on
//! [`RawSmtpTransport`] — because a TLS client is a heavy dependency the default
//! build deliberately omits; until then the transport refuses a TLS-required
//! config with a clear error rather than sending in the clear.

use std::sync::Arc;

use async_trait::async_trait;
use cellarr_core::notification::kind;
use cellarr_core::{NotificationConfig, NotificationMessage, NotificationSender};

use super::providers_support::{optional_str, required_str};

/// One outbound email, already rendered to its envelope + body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    /// The SMTP server host.
    pub host: String,
    /// The SMTP submission port.
    pub port: u16,
    /// Whether the server requires a TLS connection (implicit or STARTTLS).
    pub require_tls: bool,
    /// Optional SMTP AUTH credentials (username, password).
    pub auth: Option<(String, String)>,
    /// The envelope/from address.
    pub from: String,
    /// The recipient address(es).
    pub to: Vec<String>,
    /// The Subject header.
    pub subject: String,
    /// The plain-text body.
    pub body: String,
}

impl Email {
    /// Render the RFC 5322 message (headers + body) the `DATA` phase sends.
    #[must_use]
    pub fn render_message(&self) -> String {
        format!(
            "From: {}\r\nTo: {}\r\nSubject: {}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{}",
            self.from,
            self.to.join(", "),
            self.subject,
            self.body.replace('\n', "\r\n"),
        )
    }
}

/// The SMTP-delivery seam, so the email provider is tested against a recording
/// mock instead of a live mail server.
#[async_trait]
pub trait SmtpTransport: Send + Sync {
    /// Deliver `email`, returning `Err(detail)` on any failure (connection,
    /// auth, or a server rejection).
    async fn send(&self, email: &Email) -> Result<(), String>;
}

/// A dependency-light raw-socket SMTP submission transport (plaintext path).
pub struct RawSmtpTransport {
    timeout: std::time::Duration,
}

impl RawSmtpTransport {
    /// Build with the default bounded timeout.
    #[must_use]
    pub fn new() -> Self {
        Self {
            timeout: std::time::Duration::from_secs(20),
        }
    }
}

impl Default for RawSmtpTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SmtpTransport for RawSmtpTransport {
    async fn send(&self, email: &Email) -> Result<(), String> {
        // TODO: implicit-TLS (465) and STARTTLS submission. A TLS client is a
        // heavy dependency the default build omits, so until it lands behind a
        // feature flag this transport refuses a TLS-required config rather than
        // ever sending credentials/mail in the clear. The plaintext path below
        // covers a local relay / trusted-network submission server.
        if email.require_tls {
            return Err(
                "SMTP over TLS is not yet supported by the built-in transport (configure a \
                 plaintext submission relay, or await the TLS transport)"
                    .to_string(),
            );
        }
        let email = email.clone();
        let timeout = self.timeout;
        tokio::time::timeout(timeout, smtp_dialog(email))
            .await
            .map_err(|_| "SMTP send timed out".to_string())?
    }
}

/// Run the SMTP submission dialog over a plaintext TCP connection.
async fn smtp_dialog(email: Email) -> Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;

    let stream = TcpStream::connect((email.host.as_str(), email.port))
        .await
        .map_err(|e| format!("connect {}:{}: {e}", email.host, email.port))?;
    let (read_half, mut write) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // Read one SMTP reply line and assert it begins with `expect` (e.g. "250").
    async fn expect(
        reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
        expect: &str,
        step: &str,
    ) -> Result<(), String> {
        use tokio::io::AsyncBufReadExt;
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("read {step} reply: {e}"))?;
        if line.starts_with(expect) {
            Ok(())
        } else {
            Err(format!("SMTP {step} failed: {}", line.trim_end()))
        }
    }

    expect(&mut reader, "220", "greeting").await?;
    write
        .write_all(b"EHLO cellarr\r\n")
        .await
        .map_err(|e| format!("EHLO: {e}"))?;
    // Drain the multi-line EHLO response (lines of form "250-..." then "250 ...").
    {
        use tokio::io::AsyncBufReadExt;
        loop {
            let mut line = String::new();
            let n = reader
                .read_line(&mut line)
                .await
                .map_err(|e| format!("read EHLO reply: {e}"))?;
            if n == 0 {
                return Err("SMTP connection closed during EHLO".into());
            }
            if !line.starts_with("250") {
                return Err(format!("SMTP EHLO failed: {}", line.trim_end()));
            }
            // A space (not a hyphen) after the code marks the final line.
            if line.as_bytes().get(3) == Some(&b' ') {
                break;
            }
        }
    }

    if let Some((user, pass)) = &email.auth {
        write
            .write_all(b"AUTH LOGIN\r\n")
            .await
            .map_err(|e| format!("AUTH: {e}"))?;
        expect(&mut reader, "334", "auth-login").await?;
        write
            .write_all(format!("{}\r\n", base64_encode(user.as_bytes())).as_bytes())
            .await
            .map_err(|e| format!("auth user: {e}"))?;
        expect(&mut reader, "334", "auth-user").await?;
        write
            .write_all(format!("{}\r\n", base64_encode(pass.as_bytes())).as_bytes())
            .await
            .map_err(|e| format!("auth pass: {e}"))?;
        expect(&mut reader, "235", "auth-complete").await?;
    }

    write
        .write_all(format!("MAIL FROM:<{}>\r\n", email.from).as_bytes())
        .await
        .map_err(|e| format!("MAIL FROM: {e}"))?;
    expect(&mut reader, "250", "mail-from").await?;
    for rcpt in &email.to {
        write
            .write_all(format!("RCPT TO:<{rcpt}>\r\n").as_bytes())
            .await
            .map_err(|e| format!("RCPT TO: {e}"))?;
        expect(&mut reader, "250", "rcpt-to").await?;
    }
    write
        .write_all(b"DATA\r\n")
        .await
        .map_err(|e| format!("DATA: {e}"))?;
    expect(&mut reader, "354", "data").await?;
    let message = email.render_message();
    write
        .write_all(message.as_bytes())
        .await
        .map_err(|e| format!("message body: {e}"))?;
    write
        .write_all(b"\r\n.\r\n")
        .await
        .map_err(|e| format!("end-of-data: {e}"))?;
    expect(&mut reader, "250", "message-accept").await?;
    write.write_all(b"QUIT\r\n").await.ok();
    // Best-effort drain of the QUIT reply; failures here do not unsend the mail.
    let mut sink = [0u8; 64];
    let _ = reader.read(&mut sink).await;
    Ok(())
}

/// Standard base64 encoding (no wrapping) — the AUTH LOGIN credential encoding.
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

/// The Email / SMTP provider.
pub struct EmailSender {
    transport: Arc<dyn SmtpTransport>,
}

impl EmailSender {
    /// Build over an [`SmtpTransport`].
    #[must_use]
    pub fn new(transport: Arc<dyn SmtpTransport>) -> Self {
        Self { transport }
    }

    /// Render the [`Email`] envelope from a notification config + message. Public
    /// so the mapping is asserted directly in tests.
    pub fn build_email(
        config: &NotificationConfig,
        message: &NotificationMessage,
    ) -> Result<Email, String> {
        let host = required_str(config, "host")?.to_string();
        let port = config
            .settings
            .get("port")
            .and_then(|v| v.as_u64())
            .and_then(|p| u16::try_from(p).ok())
            .unwrap_or(587);
        let require_tls = config
            .settings
            .get("tls")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let from = required_str(config, "from")?.to_string();
        let to: Vec<String> = required_str(config, "to")?
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if to.is_empty() {
            return Err("notification setting `to` must list at least one recipient".into());
        }
        let auth = match (
            optional_str(config, "username"),
            optional_str(config, "password"),
        ) {
            (Some(u), Some(p)) => Some((u.to_string(), p.to_string())),
            _ => None,
        };
        Ok(Email {
            host,
            port,
            require_tls,
            auth,
            from,
            to,
            subject: message.title(),
            body: message.body(),
        })
    }
}

#[async_trait]
impl NotificationSender for EmailSender {
    fn kind(&self) -> &'static str {
        kind::EMAIL
    }

    async fn send(
        &self,
        config: &NotificationConfig,
        message: &NotificationMessage,
    ) -> Result<(), String> {
        let email = Self::build_email(config, message)?;
        self.transport.send(&email).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cellarr_core::NotificationEvent;
    use serde_json::json;

    #[test]
    fn base64_login_vectors() {
        assert_eq!(base64_encode(b"user"), "dXNlcg==");
        assert_eq!(base64_encode(b"pass"), "cGFzcw==");
    }

    #[test]
    fn build_email_parses_recipients_and_defaults_port() {
        let cfg = NotificationConfig {
            tags: Vec::new(),
            id: "1".into(),
            name: "mail".into(),
            kind: "email".into(),
            enabled: true,
            on_events: vec![],
            settings: json!({
                "host": "smtp.local",
                "from": "cellarr@local",
                "to": "a@local, b@local",
            }),
        };
        let msg = NotificationMessage::new(NotificationEvent::Grab, "cellarr");
        let email = EmailSender::build_email(&cfg, &msg).unwrap();
        assert_eq!(email.port, 587);
        assert_eq!(email.to, vec!["a@local".to_string(), "b@local".to_string()]);
        assert!(email
            .render_message()
            .contains("Subject: cellarr — Grabbed"));
    }
}
