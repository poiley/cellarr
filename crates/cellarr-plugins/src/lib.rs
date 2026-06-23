//! cellarr-plugins — sandboxed third-party integrations as WASM components.
//!
//! Hosts third-party extensions (custom indexers, download clients, notifiers,
//! metadata sources) as **WASM components** via [`wasmtime`], with interfaces
//! defined in **WIT**. This is the modern answer to "no stable Rust plugin ABI":
//! plugin authors aren't forced into Rust, and untrusted code runs with **no
//! ambient authority** — a plugin gets only the capabilities the host explicitly
//! grants, with CPU bounded by fuel + epoch interruption and memory by
//! `StoreLimits`.
//!
//! # Feature gating
//!
//! The wasmtime host lives behind the **`wasm`** cargo feature, which is **off by
//! default** so the lean single-binary build never pulls in Cranelift/wasmtime.
//! The capability and policy layer ([`capability`], [`config`], [`error`]) is
//! always compiled — it is small, dependency-light, and unit-testable without a
//! runtime, which is where the capability-gating and resource-limit *config*
//! tests live.
//!
//! # What is real vs. deferred
//!
//! Real (this crate):
//! - The [`indexer.wit`](../../../wit/indexer.wit) world mirroring the core
//!   [`Indexer`](cellarr_core::traits::Indexer) seam.
//! - [`IndexerPluginHost`] (feature `wasm`): compiles a component, builds a
//!   linker carrying **only** the one granted `http` import (no WASI), and
//!   instantiates under fuel + epoch + `StoreLimits`, mapping resource traps to
//!   [`PluginError::ResourceLimit`].
//! - Capability gating ([`DenyAllHttp`], [`AllowListHttp`]) and the strict,
//!   fail-closed [`HostConfig`] defaults, both unit-tested.
//!
//! Deferred (documented follow-up, not faked here):
//! - Loading and running a *real built component* end-to-end. Producing a guest
//!   needs a `cargo-component`/`wit-bindgen` toolchain build step that is too
//!   heavy to run in-tree offline; rather than ship a fake "loads a plugin"
//!   test, the host's component path is exercised by feeding it a deliberately
//!   malformed component (asserting the load error) and the gating/limit logic
//!   is tested directly. Wiring a built `.wasm` fixture through `search()` is the
//!   next step.
//! - Adapting [`IndexerPluginHost`] to `impl cellarr_core::traits::Indexer`
//!   (its methods are sync; the trait is `async`). The adapter is mechanical
//!   once a real guest exists.
//! - Worlds for `DownloadClient`, `MetadataSource`, and notifier kinds (the
//!   `Indexer` world is the template).

#![forbid(unsafe_code)]

pub mod capability;
pub mod config;
pub mod error;

pub use capability::{AllowListHttp, DenyAllHttp, HttpCapability, HttpResponse};
pub use config::HostConfig;
pub use error::{PluginError, Result};

#[cfg(feature = "wasm")]
mod host;

#[cfg(feature = "wasm")]
pub use host::IndexerPluginHost;
