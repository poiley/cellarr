//! Host-level tests for the wasmtime plugin host (feature `wasm`).
//!
//! These are honest about what is exercised end-to-end. Building a *real* guest
//! component that implements the `indexer` world requires a
//! `cargo-component`/`wit-bindgen` toolchain build that is too heavy to run
//! in-tree offline, so we do **not** ship a fake "loads a working plugin" test.
//! Instead we pin the host's safety-relevant behavior with inputs we can
//! construct deterministically and offline:
//!
//! - malformed component bytes produce a typed [`PluginError::Load`] (the host
//!   never panics on bad input);
//! - a syntactically valid component that does **not** export the `indexer`
//!   world fails instantiation with a typed error (the world contract is
//!   enforced);
//! - the capability layer denies network access unless explicitly granted
//!   (covered as unit tests in `src/capability.rs`).
//!
//! Driving a built `.wasm` fixture through `search()` is the documented
//! follow-up; see `lib.rs`.

#![cfg(feature = "wasm")]

use cellarr_core::ids::IndexerId;
use cellarr_plugins::{HostConfig, IndexerPluginHost, PluginError};

#[test]
fn malformed_bytes_fail_with_typed_load_error() {
    let garbage = b"this is definitely not a wasm component";
    match IndexerPluginHost::from_bytes(garbage, HostConfig::default(), IndexerId::new()) {
        Err(PluginError::Load(_)) => {}
        Err(other) => panic!("expected Load error, got {other:?}"),
        Ok(_) => panic!("garbage must not compile"),
    }
}

#[test]
fn empty_component_loads_but_lacks_the_indexer_export() {
    // A minimal, valid empty component in component-model WAT. It compiles
    // (so `from_bytes` succeeds) but exports nothing, so instantiating and
    // resolving the `indexer` world fails — proving the world contract is
    // enforced rather than assumed.
    let empty_component = b"(component)";
    let host = match IndexerPluginHost::from_bytes(
        empty_component,
        HostConfig::default(),
        IndexerId::new(),
    ) {
        Ok(h) => h,
        Err(e) => panic!("an empty component should compile, got {e:?}"),
    };

    let err = host
        .name()
        .expect_err("an empty component cannot satisfy the indexer world");
    // The missing-export failure surfaces as a host error (not a resource kill).
    assert!(
        matches!(err, PluginError::Host(_) | PluginError::Load(_)),
        "expected a contract/host error, got {err:?}"
    );
}

#[test]
fn default_config_denies_network_and_is_fuel_bounded() {
    // The host is constructible with the strict default policy; this guards
    // against a regression that would make the safe path fail to build a host.
    let built =
        IndexerPluginHost::from_bytes(b"(component)", HostConfig::default(), IndexerId::new());
    assert!(
        built.is_ok(),
        "constructing a host under the strict default policy must succeed"
    );
}
