//! API-key authentication.
//!
//! Mutating endpoints require a valid API key (docs/09-api.md). The key is
//! accepted either as the `X-Api-Key` header or the `apikey` query parameter —
//! the latter is what the Radarr/Sonarr ecosystem sends, so the `/api/v3` shim
//! gets the same enforcement for free. The configured key is **never logged**;
//! comparison is constant-time to avoid leaking it through timing.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use crate::error::ApiError;
use crate::state::AppState;

/// Auth configuration: the single shared API key, if one is set.
#[derive(Clone, Debug, Default)]
pub struct AuthConfig {
    /// The required API key. `None` disables auth (zero-config first run);
    /// production wiring always sets one.
    key: Option<String>,
}

impl AuthConfig {
    /// Configure with a required key.
    #[must_use]
    pub fn with_key(key: impl Into<String>) -> Self {
        Self {
            key: Some(key.into()),
        }
    }

    /// Configure with no key (auth disabled — first-run / single-user local).
    #[must_use]
    pub fn disabled() -> Self {
        Self { key: None }
    }

    /// Whether a presented key is valid. Constant-time when a key is configured.
    #[must_use]
    pub fn accepts(&self, presented: Option<&str>) -> bool {
        match &self.key {
            None => true,
            Some(expected) => presented.is_some_and(|p| constant_time_eq(p, expected)),
        }
    }
}

/// Constant-time string comparison so a wrong key can't be discovered by timing.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Extract the presented key from `X-Api-Key` or the `apikey` query parameter.
fn presented_key(req: &Request) -> Option<String> {
    if let Some(v) = req.headers().get("x-api-key").and_then(|v| v.to_str().ok()) {
        return Some(v.to_string());
    }
    req.uri().query().and_then(|q| {
        q.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == "apikey").then(|| v.to_string())
        })
    })
}

/// Middleware enforcing auth on mutating `/api/v3` routes. Reads stay open so the
/// UI and discovery work without a key on first run.
///
/// A write is admitted when EITHER:
///   * a valid API key is presented (the ecosystem path — Radarr/Sonarr clients,
///     scripts), OR
///   * the request is authenticated under the configured **web-auth** method (an
///     open install, or a logged-in Forms/Basic SPA session).
///
/// The second arm is essential: the web SPA authenticates by session and never
/// sends the API key, yet drives the v3 write endpoints (add, monitor toggle,
/// grab, …). Without it, any install that sets an API key would have a SPA that
/// can read but never write. The two arms keep ecosystem clients and the SPA both
/// working without weakening an enforced install (an anonymous caller with no key
/// and no session is still rejected).
pub async fn require_api_key(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let presented = presented_key(&req);
    if state.auth.accepts(presented.as_deref())
        || crate::webauth::request_is_web_authenticated(&state, req.headers()).await
    {
        Ok(next.run(req).await)
    } else {
        Err(ApiError::Unauthorized)
    }
}
