//! Integration tests for the web-UI authentication gate (None / Forms / Basic).
//!
//! These exercise the real router end to end over HTTP. The gate reads the auth
//! config from the live DB on every request, so each test configures the method +
//! credential by talking to the `/api/v1/auth/*` admin endpoints (or seeds the DB
//! directly) and then asserts the gate's behavior on `/api/v1`, the SPA, `/login`,
//! and — critically — that `/api/v3` stays apikey-authenticated under every method.

mod common;

use common::{start_authed, start_open, TEST_API_KEY};
use reqwest::redirect::Policy;
use reqwest::StatusCode;

/// A non-redirecting client so a Forms HTML navigation's `303 → /login` is
/// observable rather than auto-followed.
fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(Policy::none())
        .build()
        .expect("client")
}

/// Configure the single admin credential via the (open, pre-credential) admin
/// endpoint.
async fn set_credential(base: &str, username: &str, password: &str) {
    let client = reqwest::Client::new();
    let res = client
        .post(format!("{base}/api/v1/auth/credential"))
        .json(&serde_json::json!({ "username": username, "password": password }))
        .send()
        .await
        .expect("set credential");
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "credential setup should succeed"
    );
}

/// Set the auth method via the admin endpoint.
async fn set_method(base: &str, method: &str) {
    let client = reqwest::Client::new();
    let res = client
        .put(format!("{base}/api/v1/auth/config"))
        .json(&serde_json::json!({ "method": method }))
        .send()
        .await
        .expect("set method");
    assert_eq!(res.status(), StatusCode::OK, "method change should succeed");
}

// ===========================================================================
// None: fully open
// ===========================================================================

#[tokio::test]
async fn method_none_leaves_ui_and_v1_open() {
    let srv = start_open().await;
    let client = reqwest::Client::new();

    // /api/v1 read works with no credentials.
    let res = client
        .get(format!("{}/api/v1/libraries", srv.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // The SPA document is served.
    let res = client
        .get(format!("{}/", srv.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

// ===========================================================================
// Forms
// ===========================================================================

#[tokio::test]
async fn forms_login_success_sets_session_then_v1_works() {
    let srv = start_open().await;
    set_credential(&srv.base_url, "admin", "hunter2-strong").await;
    set_method(&srv.base_url, "forms").await;

    let client = no_redirect_client();

    // Before login: a gated /api/v1 read is 401 (API → JSON 401, not a redirect).
    let res = client
        .get(format!("{}/api/v1/libraries", srv.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Login with the right credentials.
    let res = client
        .post(format!("{}/login", srv.base_url))
        .json(&serde_json::json!({ "username": "admin", "password": "hunter2-strong" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cookie = res
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .expect("a session cookie is set")
        .to_str()
        .unwrap()
        .to_string();
    assert!(cookie.contains("cellarr_session="));
    assert!(cookie.contains("HttpOnly"), "cookie must be HttpOnly");
    assert!(cookie.contains("SameSite"), "cookie must set SameSite");

    // Extract the raw cookie pair to present on the next request.
    let cookie_pair = cookie.split(';').next().unwrap().to_string();

    // With the session cookie, the gated /api/v1 read now works.
    let res = client
        .get(format!("{}/api/v1/libraries", srv.base_url))
        .header(reqwest::header::COOKIE, &cookie_pair)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn forms_wrong_password_yields_401_and_no_session() {
    let srv = start_open().await;
    set_credential(&srv.base_url, "admin", "correct-horse").await;
    set_method(&srv.base_url, "forms").await;

    let client = no_redirect_client();
    let res = client
        .post(format!("{}/login", srv.base_url))
        .json(&serde_json::json!({ "username": "admin", "password": "WRONG" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(
        res.headers().get(reqwest::header::SET_COOKIE).is_none(),
        "a failed login must not mint a session cookie"
    );
}

#[tokio::test]
async fn forms_logout_invalidates_session() {
    let srv = start_open().await;
    set_credential(&srv.base_url, "admin", "correct-horse").await;
    set_method(&srv.base_url, "forms").await;

    let client = no_redirect_client();
    let res = client
        .post(format!("{}/login", srv.base_url))
        .json(&serde_json::json!({ "username": "admin", "password": "correct-horse" }))
        .send()
        .await
        .unwrap();
    let cookie_pair = res
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // Session works.
    let res = client
        .get(format!("{}/api/v1/libraries", srv.base_url))
        .header(reqwest::header::COOKIE, &cookie_pair)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Logout.
    let res = client
        .post(format!("{}/logout", srv.base_url))
        .header(reqwest::header::COOKIE, &cookie_pair)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // The same cookie is now rejected.
    let res = client
        .get(format!("{}/api/v1/libraries", srv.base_url))
        .header(reqwest::header::COOKIE, &cookie_pair)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn forms_html_navigation_redirects_to_login() {
    let srv = start_open().await;
    set_credential(&srv.base_url, "admin", "correct-horse").await;
    set_method(&srv.base_url, "forms").await;

    let client = no_redirect_client();
    // A browser navigation (Accept: text/html) to a gated page redirects.
    let res = client
        .get(format!("{}/library", srv.base_url))
        .header(reqwest::header::ACCEPT, "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers().get(reqwest::header::LOCATION).unwrap(),
        "/login"
    );

    // The login page's static assets stay reachable so the page can render.
    let res = client
        .get(format!("{}/_next/static/anything.js", srv.base_url))
        .send()
        .await
        .unwrap();
    // The asset may 404 if not built, but it must NOT be gated (no 401/redirect).
    assert!(
        res.status() == StatusCode::OK || res.status() == StatusCode::NOT_FOUND,
        "static assets must be exempt from the gate, got {}",
        res.status()
    );
}

// ===========================================================================
// Basic
// ===========================================================================

#[tokio::test]
async fn basic_challenge_then_valid_creds_pass_wrong_creds_401() {
    let srv = start_open().await;
    set_credential(&srv.base_url, "admin", "s3cr3t-pass").await;
    set_method(&srv.base_url, "basic").await;

    let client = no_redirect_client();

    // No Authorization header → 401 + WWW-Authenticate challenge.
    let res = client
        .get(format!("{}/api/v1/libraries", srv.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let challenge = res
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .expect("a Basic challenge")
        .to_str()
        .unwrap();
    assert!(challenge.contains("Basic"));
    assert!(challenge.contains("realm=\"cellarr\""));

    // Valid credentials pass.
    let res = client
        .get(format!("{}/api/v1/libraries", srv.base_url))
        .basic_auth("admin", Some("s3cr3t-pass"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Wrong password → 401.
    let res = client
        .get(format!("{}/api/v1/libraries", srv.base_url))
        .basic_auth("admin", Some("nope"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Wrong username → 401.
    let res = client
        .get(format!("{}/api/v1/libraries", srv.base_url))
        .basic_auth("root", Some("s3cr3t-pass"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

// ===========================================================================
// /api/v3 stays apikey-authenticated under EVERY method
// ===========================================================================

/// Under each web-UI method, a `/api/v3` mutating request still needs the apikey
/// (and a read still works), proving the gate never touches the *arr surface.
#[tokio::test]
async fn v3_apikey_auth_is_independent_of_web_method() {
    for method in ["none", "forms", "basic"] {
        // The server enforces the apikey on /api/v3 writes (start_authed wires the
        // apikey AuthConfig); the web method is layered on top via the DB config.
        let srv = start_authed().await;
        set_credential(&srv.base_url, "admin", "web-pass-123").await;
        set_method(&srv.base_url, method).await;

        let client = no_redirect_client();

        // A /api/v3 read (ping) is open regardless — and never gated by the web gate.
        let res = client
            .get(format!("{}/api/v3/ping", srv.base_url))
            .send()
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "v3 ping must stay open under web method {method}"
        );

        // A /api/v3 mutating request WITH the apikey → not 401 (apikey accepted).
        let res = client
            .post(format!("{}/api/v3/tag", srv.base_url))
            .header("X-Api-Key", TEST_API_KEY)
            .json(&serde_json::json!({ "label": "via-apikey" }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "v3 write WITH apikey must succeed under web method {method}"
        );

        // A /api/v3 mutating request WITHOUT the apikey → 401, under every method,
        // and it is the APIKEY 401 (never a web-gate redirect).
        let res = client
            .post(format!("{}/api/v3/tag", srv.base_url))
            .json(&serde_json::json!({ "label": "no-key" }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "v3 write WITHOUT apikey must 401 under web method {method}"
        );
    }
}

// ===========================================================================
// Stored credential is hashed, never plaintext
// ===========================================================================

#[tokio::test]
async fn password_is_stored_hashed_not_plaintext() {
    let srv = start_open().await;
    let password = "plaintext-should-not-persist";
    set_credential(&srv.base_url, "admin", password).await;

    // Inspect the persisted auth config directly through the DB.
    let cfg = srv.state.db.auth().get_config().await.unwrap();
    let stored = cfg.password_hash.expect("a hash is stored");
    assert_ne!(stored, password, "stored value must NOT be the plaintext");
    assert!(
        stored.starts_with("$argon2"),
        "stored value must be an argon2 PHC hash, got: {stored}"
    );
    // And the hash actually verifies the password (round-trip).
    assert!(cellarr_api::webauth::verify_password(password, &stored));
    assert!(!cellarr_api::webauth::verify_password("other", &stored));
}

#[tokio::test]
async fn session_tokens_are_unique_per_login() {
    let srv = start_open().await;
    set_credential(&srv.base_url, "admin", "correct-horse").await;
    set_method(&srv.base_url, "forms").await;

    let client = no_redirect_client();
    let login = || async {
        let res = client
            .post(format!("{}/login", srv.base_url))
            .json(&serde_json::json!({ "username": "admin", "password": "correct-horse" }))
            .send()
            .await
            .unwrap();
        res.headers()
            .get(reqwest::header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    };
    let a = login().await;
    let b = login().await;
    assert_ne!(a, b, "each login mints a distinct (non-constant) token");
}

// ===========================================================================
// Setup safety: selecting an enforcing method WITHOUT a credential never locks out
// ===========================================================================

#[tokio::test]
async fn enforcing_method_without_credential_stays_open_for_setup() {
    let srv = start_open().await;
    // Switch to Forms BEFORE any credential exists.
    set_method(&srv.base_url, "forms").await;

    let client = no_redirect_client();
    // The UI + /api/v1 must remain reachable so the operator can finish setup.
    let res = client
        .get(format!("{}/api/v1/libraries", srv.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "an enforcing method without a credential must not lock the operator out"
    );

    // Now set the credential — and the gate engages.
    set_credential(&srv.base_url, "admin", "now-locked-down").await;
    let res = client
        .get(format!("{}/api/v1/libraries", srv.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "once a credential exists, Forms enforces the gate"
    );
}
