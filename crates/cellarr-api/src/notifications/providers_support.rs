//! Small shared helpers for reading a notification's `settings` JSON.
//!
//! Kept in one place so every provider reports a missing required setting the
//! same way — a named `Err`, never a panic on a malformed config.

use cellarr_core::NotificationConfig;
use serde_json::Value;

/// Read a required string setting, or an `Err` naming the missing key.
pub fn required_str<'a>(config: &'a NotificationConfig, key: &str) -> Result<&'a str, String> {
    config
        .settings
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("notification setting `{key}` is required"))
}

/// Read an optional string setting (absent or empty -> `None`).
pub fn optional_str<'a>(config: &'a NotificationConfig, key: &str) -> Option<&'a str> {
    config
        .settings
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}
