//! Config precedence: built-in defaults < config file < environment.
//!
//! Pins the layering contract from `docs/01-architecture.md` /
//! `docs/specs/cellarr-cli.md`. Env vars are process-global, so this whole file
//! runs as **one** test that sets/clears them in sequence — running the cases as
//! separate `#[test]`s would race the shared env under parallel execution.

use std::io::Write;

use cellarr_cli::config::{Config, DEFAULT_API_PORT};

/// A unique env var name per logical key so concurrent test *binaries* (other
/// crates) cannot collide — though within this file we serialize anyway.
fn write_config(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cellarr.toml");
    let mut f = std::fs::File::create(&path).expect("create config");
    f.write_all(body.as_bytes()).expect("write config");
    (dir, path)
}

#[test]
fn defaults_then_file_then_env() {
    // --- 1. Defaults alone (zero-config) -----------------------------------
    // No file, no env: the built-in defaults must fully configure the daemon.
    clear_env();
    let defaults = Config::load(None).expect("defaults load");
    assert_eq!(
        defaults.api.port, DEFAULT_API_PORT,
        "default port is the built-in"
    );
    assert!(
        defaults.api.api_key.is_none(),
        "auth disabled by default (zero-config first run)"
    );
    assert!(!defaults.metrics.enabled, "metrics off by default");
    assert_eq!(
        defaults.api.bind.to_string(),
        "127.0.0.1",
        "loopback default"
    );

    // --- 2. File overrides a default ---------------------------------------
    let (_dir, path) = write_config(
        r#"
[api]
port = 12345
api_key = "from-file"

[log]
filter = "debug"
"#,
    );
    clear_env();
    let with_file = Config::load(Some(&path)).expect("file load");
    assert_eq!(with_file.api.port, 12345, "file overrides the default port");
    assert_eq!(
        with_file.api.api_key.as_deref(),
        Some("from-file"),
        "file sets the api key"
    );
    assert_eq!(
        with_file.log.filter, "debug",
        "file overrides the log filter"
    );
    // A key the file does not set keeps its default.
    assert_eq!(
        with_file.api.bind.to_string(),
        "127.0.0.1",
        "unset key keeps the default"
    );

    // --- 3. Env overrides the file -----------------------------------------
    std::env::set_var("CELLARR_API__PORT", "23456");
    std::env::set_var("CELLARR_API__API_KEY", "from-env");
    let with_env = Config::load(Some(&path)).expect("env load");
    assert_eq!(
        with_env.api.port, 23456,
        "env wins over the file for the port"
    );
    assert_eq!(
        with_env.api.api_key.as_deref(),
        Some("from-env"),
        "env wins over the file for the key"
    );
    // A key only the file sets is still in effect under the env layer.
    assert_eq!(
        with_env.log.filter, "debug",
        "file value survives where env is silent"
    );

    clear_env();
}

fn clear_env() {
    for k in [
        "CELLARR_API__PORT",
        "CELLARR_API__API_KEY",
        "CELLARR_API__BIND",
    ] {
        std::env::remove_var(k);
    }
}
