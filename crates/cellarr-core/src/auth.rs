//! Web-UI authentication configuration (the single-admin model).
//!
//! cellarr mirrors Sonarr/Radarr's "Authentication" setting, minus multi-user:
//! there is **one** admin account (a username plus a password *hash*) and a chosen
//! [`AuthMethod`]. This module holds only the *persisted shapes* — the method enum
//! and the credential record — plus the pure helpers for reasoning about them.
//! No I/O, no hashing, no HTTP lives here (this crate is pure types): the password
//! is hashed in the API crate (which owns the Argon2 dependency) and the record is
//! stored by `cellarr-db`. The plaintext password is **never** modelled here, so it
//! cannot accidentally be persisted or logged.
//!
//! The gate this configures applies to the web UI and the UI's private `/api/v1`
//! surface; it deliberately does **not** govern `/api/v3` (which stays apikey-
//! authenticated for the *arr ecosystem). See `docs/09-api.md`.

use serde::{Deserialize, Serialize};

/// How the web UI + `/api/v1` authenticate the single admin.
///
/// Mirrors Sonarr/Radarr's Authentication dropdown (minus the multi-user
/// "Forms (login page)" vs "External" distinctions): exactly the three modes a
/// single-user install needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    /// No authentication — the UI and `/api/v1` are open. The zero-config
    /// first-run default so a fresh install is usable immediately on loopback.
    #[default]
    None,
    /// Forms authentication: a `/login` page mints a session cookie; subsequent
    /// requests are authorized by that cookie.
    Forms,
    /// HTTP Basic authentication: every request carries
    /// `Authorization: Basic <base64(user:pass)>`, challenged with a `401` +
    /// `WWW-Authenticate` when absent/invalid.
    Basic,
}

impl AuthMethod {
    /// The stable lowercase wire token for this method (the value the get/set
    /// auth-config API round-trips, and what the v3 `system/status`
    /// `authentication` field can reflect).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AuthMethod::None => "none",
            AuthMethod::Forms => "forms",
            AuthMethod::Basic => "basic",
        }
    }

    /// Whether this method enforces a credential gate at all (anything but
    /// [`None`](AuthMethod::None)).
    #[must_use]
    pub fn is_enforced(self) -> bool {
        !matches!(self, AuthMethod::None)
    }
}

/// The persisted single-admin authentication configuration.
///
/// Exactly one logical record exists (the install's auth settings). It carries the
/// chosen [`AuthMethod`] and — once the admin has been set up — the admin
/// `username` and the **password hash** (an Argon2 PHC string produced by the API
/// crate). `username`/`password_hash` are `None` until setup, which is the state
/// that lets [`needs_setup`](Self::needs_setup) keep an operator from locking
/// themselves out by selecting Forms/Basic before a credential exists.
///
/// The plaintext password is never a field here, so it can never be serialized to
/// the database or to a log line.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthConfig {
    /// The enforced method. Defaults to [`AuthMethod::None`] (open).
    #[serde(default)]
    pub method: AuthMethod,
    /// The single admin's username. `None` until a credential is set up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// The admin password **hash** (an Argon2 PHC string). `None` until a
    /// credential is set up. Never the plaintext; never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
}

impl AuthConfig {
    /// An open configuration (no method, no credential) — the zero-config default.
    #[must_use]
    pub fn open() -> Self {
        Self::default()
    }

    /// Whether a credential (username + password hash) has been configured.
    #[must_use]
    pub fn has_credential(&self) -> bool {
        self.username.is_some() && self.password_hash.is_some()
    }

    /// Whether the configuration would lock the operator out: an enforcing method
    /// is selected but **no credential exists yet**. The setup flow uses this to
    /// guide the user to set a credential rather than enforce an unusable gate
    /// (selecting Forms/Basic with no admin would otherwise make every request —
    /// including the one that *sets* the credential — fail).
    #[must_use]
    pub fn needs_setup(&self) -> bool {
        self.method.is_enforced() && !self.has_credential()
    }

    /// Whether the gate is *effectively* enforced right now: the method enforces
    /// **and** a credential exists to check against. A method selected without a
    /// credential is not effectively enforced — the middleware passes through so
    /// the operator can finish setup (it never silently locks the UI).
    #[must_use]
    pub fn is_effectively_enforced(&self) -> bool {
        self.method.is_enforced() && self.has_credential()
    }

    /// Whether `username` matches the configured admin username. Case-sensitive
    /// (usernames are exact), and always `false` before a credential is set up.
    #[must_use]
    pub fn username_matches(&self, username: &str) -> bool {
        self.username.as_deref() == Some(username)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_round_trips_through_lowercase_token() {
        for m in [AuthMethod::None, AuthMethod::Forms, AuthMethod::Basic] {
            let json = serde_json::to_value(m).unwrap();
            assert_eq!(json, serde_json::Value::String(m.as_str().to_string()));
            let back: AuthMethod = serde_json::from_value(json).unwrap();
            assert_eq!(m, back);
        }
    }

    #[test]
    fn default_is_open_and_not_enforced() {
        let cfg = AuthConfig::default();
        assert_eq!(cfg.method, AuthMethod::None);
        assert!(!cfg.has_credential());
        assert!(!cfg.needs_setup());
        assert!(!cfg.is_effectively_enforced());
    }

    #[test]
    fn enforcing_without_credential_needs_setup_and_is_not_enforced() {
        let cfg = AuthConfig {
            method: AuthMethod::Forms,
            username: None,
            password_hash: None,
        };
        assert!(cfg.needs_setup());
        // Crucially NOT effectively enforced: the middleware must pass through so
        // the operator can still reach the UI to finish setup.
        assert!(!cfg.is_effectively_enforced());
    }

    #[test]
    fn enforcing_with_credential_is_enforced() {
        let cfg = AuthConfig {
            method: AuthMethod::Basic,
            username: Some("admin".into()),
            password_hash: Some("$argon2id$...".into()),
        };
        assert!(!cfg.needs_setup());
        assert!(cfg.is_effectively_enforced());
        assert!(cfg.username_matches("admin"));
        assert!(!cfg.username_matches("root"));
    }

    #[test]
    fn password_plaintext_is_never_a_field() {
        // Guard: the serialized form must only ever carry the hash, never a
        // plaintext password key. (Compile-time-ish: this asserts the shape.)
        let cfg = AuthConfig {
            method: AuthMethod::Forms,
            username: Some("admin".into()),
            password_hash: Some("HASH".into()),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("passwordHash"));
        assert!(!json.to_lowercase().contains("\"password\""));
    }
}
