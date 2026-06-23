//! The HTTP [`WebhookSender`] implementation.
//!
//! `cellarr-core` defines the [`WebhookSender`] seam (POST a [`WebhookPayload`]
//! to a URL); this is the live `reqwest` delivery the daemon wires in. It is kept
//! out of `cellarr-core` (which does no I/O) and the pipeline crate (no HTTP
//! client) and lives here, where the API already depends on `reqwest`.
//!
//! Delivery is bounded by a short timeout and best-effort: a failure is returned
//! as `Err(detail)`, which the dispatcher logs and swallows so a dead receiver
//! never breaks the pipeline.

use std::time::Duration;

use async_trait::async_trait;
use cellarr_core::{WebhookPayload, WebhookSender};

/// A `reqwest`-backed [`WebhookSender`].
#[derive(Clone)]
pub struct ReqwestWebhookSender {
    client: reqwest::Client,
}

impl ReqwestWebhookSender {
    /// Build a sender with a bounded per-request timeout (so a hung receiver does
    /// not stall the pipeline).
    #[must_use]
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl Default for ReqwestWebhookSender {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WebhookSender for ReqwestWebhookSender {
    async fn send(&self, url: &str, payload: &WebhookPayload) -> Result<(), String> {
        let resp = self
            .client
            .post(url)
            .json(payload)
            .send()
            .await
            .map_err(|e| format!("webhook POST to {url} failed: {e}"))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!(
                "webhook POST to {url} returned status {}",
                resp.status()
            ))
        }
    }
}
