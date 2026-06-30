//! Web-UI authentication: the single-admin gate over the UI + `/api/v1`.
//!
//! This is **separate** from the apikey auth in [`crate::auth`]: that one guards
//! the `/api/v3` *arr-compatibility surface and stays exactly as is. This module
//! implements the user-facing login gate that mirrors Sonarr/Radarr's
//! "Authentication" setting (minus multi-user): one admin (username + password
//! hash) and a method of `None | Forms | Basic` ([`cellarr_core::AuthMethod`]).
//!
//! ## What it gates
//! The [`gate`] middleware is applied to the whole router and decides per request:
//! - `/api/v3` (and the `/sonarr|radarr/api/v3` faces, the calendar feed) — **never**
//!   gated here; the apikey middleware owns them.
//! - `/login`, `/logout`, `/health`, and the static assets the login page needs —
//!   always reachable, so the operator can authenticate / a fresh install can be set up.
//! - everything else (the SPA + `/api/v1`) — gated per the configured method.
//!
//! ## Methods
//! - **None**: pass through.
//! - **Basic**: require `Authorization: Basic base64(user:pass)` matching the admin
//!   (constant-time-ish username + Argon2 password verify); else `401` +
//!   `WWW-Authenticate: Basic realm="cellarr"`.
//! - **Forms**: require a valid session cookie; missing/invalid → `401` for an API
//!   request, or a redirect to `/login` for an HTML navigation. `POST /login`
//!   verifies the password and mints a CSPRNG session token in an HttpOnly cookie;
//!   `POST /logout` invalidates it.
//!
//! ## Safety
//! A method selected with **no credential yet** is not effectively enforced
//! ([`cellarr_core::AuthConfig::is_effectively_enforced`]), so the operator can
//! never lock themselves out before setting an admin. Passwords are stored as an
//! Argon2 PHC hash and never logged; session tokens come from the OS CSPRNG.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use cellarr_core::{AuthConfig, AuthMethod};
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// The session cookie name. Distinct/namespaced so it never collides with an
/// unrelated cookie on the same origin.
const SESSION_COOKIE: &str = "cellarr_session";

/// How long a Forms session lives, in seconds (7 days). After this the cookie is
/// rejected and the operator logs in again.
const SESSION_TTL_SECS: i64 = 7 * 24 * 60 * 60;

/// The number of random bytes in a session token before base64url encoding.
/// 32 bytes = 256 bits of CSPRNG entropy — unguessable.
const SESSION_TOKEN_BYTES: usize = 32;

/// The Basic-auth realm advertised in the `WWW-Authenticate` challenge.
const BASIC_REALM: &str = "cellarr";

// ===========================================================================
// Password hashing (Argon2) + CSPRNG tokens
// ===========================================================================

/// Hash a plaintext password into an Argon2id PHC string (salt embedded). The
/// plaintext is consumed by reference and never stored or logged. Returns an
/// opaque error string (never echoing the password) on the rare hashing failure.
///
/// # Errors
/// Returns an error if Argon2 hashing fails (e.g. an out-of-memory params error).
pub fn hash_password(plaintext: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plaintext.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("password hash failed: {e}"))
}

/// Verify a plaintext password against a stored Argon2 PHC hash. Returns `false`
/// for any mismatch or a malformed stored hash (never panics, never logs the
/// password). Argon2 verification is itself constant-time over the derived hash.
#[must_use]
pub fn verify_password(plaintext: &str, phc_hash: &str) -> bool {
    match PasswordHash::new(phc_hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(plaintext.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// Generate a fresh, unguessable session token: 256 bits of OS CSPRNG entropy,
/// URL-safe-base64 encoded (no padding) so it is a clean cookie value.
#[must_use]
pub fn generate_session_token() -> String {
    let mut bytes = [0u8; SESSION_TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    base64url_nopad(&bytes)
}

/// Minimal URL-safe base64 (no padding) encoder, avoiding a base64 crate dep for
/// the one place we need it (session tokens + Basic decode reuse the std-free
/// table here for encode; decode is separate below).
fn base64url_nopad(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((input.len() * 4).div_ceil(3));
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(n & 0x3f) as usize] as char);
        }
    }
    out
}

/// Constant-time byte-slice equality, so a wrong username can't be discovered by
/// timing. (The password side is constant-time inside Argon2's verify.)
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Standard base64 decode (the `+`/`/` alphabet, padding optional) for the
/// `Authorization: Basic` value. Returns `None` on any invalid input rather than
/// erroring, so a malformed header is simply "no valid credentials".
fn base64_std_decode(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let cleaned: Vec<u8> = input.bytes().filter(|&c| c != b'=').collect();
    let mut out = Vec::with_capacity(cleaned.len() * 3 / 4);
    for chunk in cleaned.chunks(4) {
        let mut n = 0u32;
        let mut bits = 0u32;
        for &c in chunk {
            n = (n << 6) | val(c)?;
            bits += 6;
        }
        // Left-justify the accumulated bits, then emit whole bytes.
        n <<= 24 - bits;
        let bytes = (bits / 8) as usize;
        for i in 0..bytes {
            out.push(((n >> (16 - i * 8)) & 0xff) as u8);
        }
    }
    Some(out)
}

// ===========================================================================
// The gate middleware
// ===========================================================================

/// Whether a request path is exempt from the web-UI gate.
///
/// Exempt: the `/api/v3` apikey surfaces (cellarr, sonarr, radarr faces + the
/// calendar feed), the login/logout endpoints, the top-level health probe, and
/// the static assets the login page itself needs (so the login screen can render
/// for an unauthenticated user).
fn is_exempt(path: &str) -> bool {
    // The apikey-authenticated *arr surface — never gated here.
    if path == "/api/v3"
        || path.starts_with("/api/v3/")
        || path.starts_with("/sonarr/api/v3")
        || path.starts_with("/radarr/api/v3")
        || path.starts_with("/feed/")
    {
        return true;
    }
    // Auth endpoints + health probe always reachable. The trailing-slash forms
    // are exempt too so a browser navigation to `/login/` serves the login page
    // rather than 303-redirecting back to `/login` (a redirect loop).
    if path == "/login"
        || path == "/login/"
        || path == "/logout"
        || path == "/logout/"
        || path == "/health"
    {
        return true;
    }
    // Static assets the login page pulls in (Next.js export emits these), plus
    // common root files. Gating these would blank out the login screen.
    if path.starts_with("/_next/")
        || path == "/favicon.ico"
        || path.starts_with("/static/")
        || path.starts_with("/assets/")
    {
        return true;
    }
    // Any path with a static-asset file extension is a resource the login page may
    // reference (css/js/fonts/images). HTML documents are NOT exempt (so a
    // gated page redirects to /login).
    if let Some(seg) = path.rsplit('/').next() {
        if let Some((_, ext)) = seg.rsplit_once('.') {
            return matches!(
                ext,
                "js" | "mjs"
                    | "css"
                    | "map"
                    | "svg"
                    | "png"
                    | "jpg"
                    | "jpeg"
                    | "webp"
                    | "ico"
                    | "woff"
                    | "woff2"
                    | "ttf"
                    | "json"
                    | "txt"
            );
        }
    }
    false
}

/// Whether a request looks like a browser navigation (wants HTML) vs an API/XHR
/// call (wants JSON). Drives Forms behavior: a navigation gets a `303` redirect to
/// `/login`; an API call gets a `401` so the client can branch on it.
fn wants_html(headers: &HeaderMap, path: &str) -> bool {
    // An `/api/` request is always treated as an API call (JSON 401), regardless
    // of Accept, so XHR clients never receive a redirect.
    if path.starts_with("/api/") {
        return false;
    }
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/html"))
}

/// Extract the session cookie value from the `Cookie` header, if present.
fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix(SESSION_COOKIE) {
            if let Some(val) = rest.strip_prefix('=') {
                return Some(val.to_string());
            }
        }
    }
    None
}

/// Parse `Authorization: Basic <b64>` into `(username, password)`.
fn basic_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let b64 = raw
        .strip_prefix("Basic ")
        .or_else(|| raw.strip_prefix("basic "))?;
    let decoded = base64_std_decode(b64.trim())?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, pass) = text.split_once(':')?;
    Some((user.to_string(), pass.to_string()))
}

/// Now, as unix seconds (saturating; never panics).
fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Whether a request is authenticated under the configured **web-auth** method,
/// independent of the `/api/v3` apikey surface.
///
/// Returns `true` when the method is not effectively enforced (an open install),
/// or a valid Basic credential / Forms session is present. This is the seam the
/// `/api/v3` apikey middleware uses to *also* admit a logged-in SPA user (or any
/// caller on an open install): the web UI authenticates by session and never sends
/// the apikey, so without this it could not drive the v3 write endpoints the SPA
/// depends on (add, monitor toggle, grab, …). A config read failure fails closed.
pub async fn request_is_web_authenticated(state: &AppState, headers: &HeaderMap) -> bool {
    let Ok(cfg) = state.db.auth().get_config().await else {
        return false;
    };
    if !cfg.is_effectively_enforced() {
        return true;
    }
    match cfg.method {
        AuthMethod::None => true,
        AuthMethod::Basic => check_basic(headers, &cfg),
        AuthMethod::Forms => check_forms(state, headers).await,
    }
}

/// The web-UI authentication gate. Applied to the whole router; exempt paths
/// (`/api/v3`, `/login`, `/health`, static assets) pass straight through, and the
/// configured [`AuthMethod`] enforces the rest.
pub async fn gate(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    if is_exempt(&path) {
        return next.run(req).await;
    }

    // Read the live auth config. A read failure must fail closed for an enforced
    // method, but the open default means a DB hiccup on a None install never locks
    // the UI; we treat a config read error as "deny" only conservatively below.
    let cfg = match state.db.auth().get_config().await {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::error!(error = %e, "reading auth config in gate");
            // Fail closed: if we cannot read the config we cannot prove the request
            // is allowed. Return a generic 401 (never leak the DB error).
            return ApiError::Unauthorized.into_response();
        }
    };

    // Not effectively enforced (None, or a method without a credential yet): open.
    if !cfg.is_effectively_enforced() {
        return next.run(req).await;
    }

    match cfg.method {
        AuthMethod::None => next.run(req).await,
        AuthMethod::Basic => match check_basic(req.headers(), &cfg) {
            true => next.run(req).await,
            false => basic_challenge(),
        },
        AuthMethod::Forms => {
            if check_forms(&state, req.headers()).await {
                next.run(req).await
            } else if wants_html(req.headers(), &path) {
                // A browser navigation → redirect to the login page.
                redirect_to_login()
            } else {
                ApiError::Unauthorized.into_response()
            }
        }
    }
}

/// Verify a Basic-auth request against the configured admin (constant-time
/// username compare + Argon2 password verify).
fn check_basic(headers: &HeaderMap, cfg: &AuthConfig) -> bool {
    let Some((user, pass)) = basic_credentials(headers) else {
        return false;
    };
    let (Some(expected_user), Some(hash)) = (cfg.username.as_deref(), cfg.password_hash.as_deref())
    else {
        return false;
    };
    // Constant-time username compare, then the (constant-time) password verify.
    // Both run regardless of the username result so a wrong username and a wrong
    // password take the same path.
    let user_ok = constant_time_eq(user.as_bytes(), expected_user.as_bytes());
    let pass_ok = verify_password(&pass, hash);
    user_ok && pass_ok
}

/// Verify a Forms request by its session cookie against the live session store.
async fn check_forms(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(token) = session_cookie(headers) else {
        return false;
    };
    matches!(
        state.db.auth().get_session(&token, now_unix()).await,
        Ok(Some(_))
    )
}

/// The `401` Basic challenge with the `WWW-Authenticate` header naming the realm.
fn basic_challenge() -> Response {
    let mut resp = ApiError::Unauthorized.into_response();
    let challenge = format!("Basic realm=\"{BASIC_REALM}\"");
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        resp.headers_mut().insert(header::WWW_AUTHENTICATE, value);
    }
    resp
}

/// A `303 See Other` redirect to the login page for an HTML navigation.
fn redirect_to_login() -> Response {
    (
        StatusCode::SEE_OTHER,
        [(header::LOCATION, HeaderValue::from_static("/login"))],
    )
        .into_response()
}

// ===========================================================================
// Login / logout + auth-config endpoints
// ===========================================================================

/// Build the unauthenticated login/logout router (`/login`, `/logout`). Merged at
/// the top level so it sits *outside* the v1 apikey middleware and the gate
/// exempts it explicitly.
///
/// `/login` answers **both** methods: `GET` serves the embedded SRCL login page
/// (so a browser sent here by the Forms 303 redirect actually sees the form),
/// and `POST` performs the credential check + mints the session. Without the
/// `GET` arm the bare `POST` route would shadow the asset fallback and a browser
/// navigation to `/login` would get a `405 Method Not Allowed` instead of the
/// login screen.
pub fn auth_routes(state: AppState) -> Router {
    Router::new()
        .route("/login", post(login).get(serve_login))
        .route("/logout", post(logout))
        .with_state(state)
}

/// `GET /login` — serve the embedded login page. Delegates to the asset server,
/// whose extensionless-route rule maps `/login` to the exported
/// `login/index.html`. This keeps the login screen reachable for an
/// unauthenticated browser (the Forms gate redirects HTML navigations here).
async fn serve_login(uri: axum::http::Uri) -> Response {
    crate::assets::serve(uri).await
}

/// The login request body.
#[derive(Debug, Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

/// The login/logout/auth-config success envelope the UI reads.
#[derive(Debug, Serialize)]
struct AuthStatus {
    /// The active method (`none` | `forms` | `basic`).
    method: &'static str,
    /// Whether a credential (admin user) has been configured.
    configured: bool,
    /// Whether the gate is effectively enforced right now.
    enforced: bool,
    /// The admin username, when configured (never the hash).
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
}

impl AuthStatus {
    fn from_config(cfg: &AuthConfig) -> Self {
        Self {
            method: cfg.method.as_str(),
            configured: cfg.has_credential(),
            enforced: cfg.is_effectively_enforced(),
            username: cfg.username.clone(),
        }
    }
}

/// `POST /login` — verify the admin credential and, on success, mint a session
/// cookie. Works under any method (it is how Forms authenticates; under Basic/None
/// it still validates the credential, which the UI uses to confirm a password).
/// A wrong username or password returns `401` with **no** session and no oracle as
/// to which was wrong.
async fn login(State(state): State<AppState>, Json(body): Json<LoginBody>) -> ApiResult<Response> {
    let cfg = state.db.auth().get_config().await?;
    let (Some(expected_user), Some(hash)) = (cfg.username.as_deref(), cfg.password_hash.as_deref())
    else {
        // No credential configured: nothing to log into.
        return Err(ApiError::Unauthorized);
    };
    let user_ok = constant_time_eq(body.username.as_bytes(), expected_user.as_bytes());
    let pass_ok = verify_password(&body.password, hash);
    if !(user_ok && pass_ok) {
        return Err(ApiError::Unauthorized);
    }

    // Mint a CSPRNG session token and persist it with an expiry.
    let token = generate_session_token();
    let now = now_unix();
    let expires = now.saturating_add(SESSION_TTL_SECS);
    state
        .db
        .auth()
        .create_session(&token, expected_user, now, expires)
        .await?;

    let status = AuthStatus::from_config(&cfg);
    let cookie = session_set_cookie(&token, SESSION_TTL_SECS, cookie_secure());
    let mut resp = Json(status).into_response();
    resp.headers_mut().insert(header::SET_COOKIE, cookie);
    Ok(resp)
}

/// `POST /logout` — invalidate the presented session (if any) and clear the
/// cookie. Idempotent: logging out with no/expired session still returns `200`.
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(token) = session_cookie(&headers) {
        let _ = state.db.auth().delete_session(&token).await?;
    }
    let mut resp = Json(serde_json::json!({ "ok": true })).into_response();
    resp.headers_mut()
        .insert(header::SET_COOKIE, session_clear_cookie());
    Ok(resp)
}

/// Build a `Set-Cookie` value for the session: HttpOnly, SameSite=Lax, Path=/,
/// `Max-Age`, and `Secure` when served over TLS.
fn session_set_cookie(token: &str, max_age: i64, secure: bool) -> HeaderValue {
    let mut v =
        format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}");
    if secure {
        v.push_str("; Secure");
    }
    // The token is base64url (cookie-safe) and the rest is static, so this never
    // fails; fall back to a benign clearing cookie if it somehow does.
    HeaderValue::from_str(&v).unwrap_or_else(|_| session_clear_cookie())
}

/// A `Set-Cookie` value that immediately expires the session cookie.
fn session_clear_cookie() -> HeaderValue {
    HeaderValue::from_static("cellarr_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
}

/// Whether the session cookie should carry the `Secure` attribute.
///
/// The daemon binds plain HTTP locally by default (loopback); a reverse proxy
/// terminating TLS is the common remote deployment. Setting `Secure` on the
/// plain-HTTP local case would *break* login (the browser would drop the cookie),
/// so this is conservatively `false`. Deriving it from `X-Forwarded-Proto` behind
/// a TLS-terminating proxy is a documented follow-up.
// TODO(webauth): set Secure when the request arrived over TLS (read
// X-Forwarded-Proto / the connection scheme) so the cookie is hardened for HTTPS
// deployments while staying usable on plain-HTTP loopback.
fn cookie_secure() -> bool {
    false
}

// ===========================================================================
// Auth-config admin endpoints (mounted under the gated /api/v1)
// ===========================================================================

/// Build the auth-config admin router. These routes are merged into `/api/v1`, so
/// once a credential exists they sit behind the gate (admin-only). Before a
/// credential exists the install is open, so the first setup call is reachable —
/// this is what prevents lockout when switching to Forms/Basic.
pub fn config_routes(state: AppState) -> Router {
    Router::new()
        .route("/auth/config", axum::routing::get(get_auth_config))
        .route("/auth/config", axum::routing::put(set_auth_method))
        .route("/auth/credential", post(set_credential))
        .with_state(state)
}

/// `GET /api/v1/auth/config` — the current auth status (method, whether a
/// credential is configured, whether enforced, the username). Never returns the
/// password hash.
async fn get_auth_config(State(state): State<AppState>) -> ApiResult<Json<AuthStatus>> {
    let cfg = state.db.auth().get_config().await?;
    Ok(Json(AuthStatus::from_config(&cfg)))
}

/// Body for setting the auth method.
#[derive(Debug, Deserialize)]
struct SetMethodBody {
    method: AuthMethod,
}

/// `PUT /api/v1/auth/config` — set the authentication method. Selecting an
/// enforcing method (`forms`/`basic`) with **no credential yet** is allowed but
/// flagged via the returned `enforced:false` + `configured:false` so the UI guides
/// the operator to set a credential next (the gate stays open until they do, so
/// this call can never lock them out).
async fn set_auth_method(
    State(state): State<AppState>,
    Json(body): Json<SetMethodBody>,
) -> ApiResult<Json<AuthStatus>> {
    let mut cfg = state.db.auth().get_config().await?;
    let changed = cfg.method != body.method;
    cfg.method = body.method;
    state.db.auth().set_config(&cfg).await?;
    // Switching method invalidates any live Forms sessions, so a method change is
    // a clean re-auth boundary.
    if changed {
        state.db.auth().delete_all_sessions().await?;
    }
    Ok(Json(AuthStatus::from_config(&cfg)))
}

/// Body for setting/changing the admin credential.
#[derive(Debug, Deserialize)]
struct CredentialBody {
    username: String,
    password: String,
}

/// `POST /api/v1/auth/credential` — set or change the single admin's username and
/// password. The password is Argon2-hashed before storage (never persisted or
/// logged in plaintext). Changing the credential revokes all existing sessions.
///
/// Empty username/password are rejected (`400`) so a credential is never set to a
/// trivially-bypassable empty value.
async fn set_credential(
    State(state): State<AppState>,
    Json(body): Json<CredentialBody>,
) -> ApiResult<Json<AuthStatus>> {
    if body.username.trim().is_empty() {
        return Err(ApiError::BadRequest("username is required".into()));
    }
    if body.password.is_empty() {
        return Err(ApiError::BadRequest("password is required".into()));
    }
    let hash = hash_password(&body.password).map_err(ApiError::Internal)?;
    let mut cfg = state.db.auth().get_config().await?;
    cfg.username = Some(body.username.trim().to_string());
    cfg.password_hash = Some(hash);
    state.db.auth().set_config(&cfg).await?;
    // A credential change invalidates any prior sessions (they were minted under
    // the old credential).
    state.db.auth().delete_all_sessions().await?;
    Ok(Json(AuthStatus::from_config(&cfg)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_round_trips_and_is_not_plaintext() {
        let hash = hash_password("hunter2").unwrap();
        assert_ne!(hash, "hunter2", "stored value must not be the plaintext");
        assert!(hash.starts_with("$argon2"), "must be an argon2 PHC string");
        assert!(verify_password("hunter2", &hash));
        assert!(!verify_password("wrong", &hash));
    }

    #[test]
    fn verify_rejects_malformed_hash_without_panicking() {
        assert!(!verify_password("x", "not-a-phc-string"));
        assert!(!verify_password("x", ""));
    }

    #[test]
    fn session_tokens_are_unique_and_long() {
        let a = generate_session_token();
        let b = generate_session_token();
        assert_ne!(a, b, "tokens must not be a static constant");
        // 32 bytes base64url ≈ 43 chars.
        assert!(a.len() >= 40, "token should carry full entropy");
    }

    #[test]
    fn base64_decode_handles_basic_credentials() {
        // "admin:secret" -> base64
        let encoded = "YWRtaW46c2VjcmV0";
        let decoded = base64_std_decode(encoded).unwrap();
        assert_eq!(decoded, b"admin:secret");
    }

    #[test]
    fn base64url_encode_matches_known_vector() {
        // RFC 4648 url-safe, no pad. "foobar" stays alnum so this is a sanity check
        // of the chunking.
        assert_eq!(base64url_nopad(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64url_nopad(b"fo"), "Zm8");
    }

    #[test]
    fn exempt_paths_cover_v3_login_health_and_assets() {
        assert!(is_exempt("/api/v3/movie"));
        assert!(is_exempt("/sonarr/api/v3/series"));
        assert!(is_exempt("/radarr/api/v3/movie"));
        assert!(is_exempt("/feed/v3/calendar/radarr.ics"));
        assert!(is_exempt("/login"));
        assert!(is_exempt("/login/"));
        assert!(is_exempt("/logout"));
        assert!(is_exempt("/logout/"));
        assert!(is_exempt("/health"));
        assert!(is_exempt("/_next/static/app.js"));
        assert!(is_exempt("/favicon.ico"));
        assert!(is_exempt("/main.css"));
        // Gated: the SPA documents and the private v1 API.
        assert!(!is_exempt("/api/v1/libraries"));
        assert!(!is_exempt("/library"));
        assert!(!is_exempt("/"));
    }

    #[test]
    fn constant_time_eq_matches_semantics() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
