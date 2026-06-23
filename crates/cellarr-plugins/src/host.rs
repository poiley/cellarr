//! The wasmtime Component-Model host (behind the `wasm` feature).
//!
//! [`IndexerPluginHost`] compiles a WASM component implementing the
//! `cellarr:plugins/indexer-plugin` world, instantiates it under strict
//! resource limits, grants only the explicitly configured capabilities, and
//! adapts the result to the shapes the rest of cellarr speaks.
//!
//! Design notes:
//! - **No ambient authority.** The linker is built empty and given exactly one
//!   import — `cellarr:plugins/http` — wired to either the granted
//!   [`HttpCapability`] or a deny-all stub depending on [`HostConfig::grant_http`].
//!   No WASI, no sockets, no clocks, no filesystem are added.
//! - **Three resource controls.** Fuel (via `Config::consume_fuel` +
//!   `Store::set_fuel`), epoch interruption (via `Config::epoch_interruption` +
//!   `Store::set_epoch_deadline`, ticked by a background thread), and memory via
//!   `StoreLimits`. A guest that exhausts any of them traps; the host maps the
//!   trap to [`PluginError::ResourceLimit`] and drops the instance, so a
//!   misbehaving plugin cannot affect the daemon.
//! - **Fresh instance per invocation.** Each call builds a new `Store` and
//!   instance; `max_instances` is 1. This is the spec's posture for untrusted
//!   code — no state survives between calls.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

use cellarr_core::ids::IndexerId;
use cellarr_core::release::{Protocol, Release};
use cellarr_core::traits::SearchTerms;

use crate::capability::{DenyAllHttp, HttpCapability};
use crate::config::HostConfig;
use crate::error::{PluginError, Result};

// Generate host/guest bindings from the WIT world. The host implements the
// imported `http` interface; the guest implements the exported `indexer`.
// WASIp2 sync per the spec's default posture — no async.
wasmtime::component::bindgen!({
    path: "wit/indexer.wit",
    world: "indexer-plugin",
});

use cellarr::plugins::http::{Host as HttpHost, HttpResponse as WitHttpResponse};
use cellarr::plugins::types::{
    PluginError as WitPluginError, Protocol as WitProtocol, Release as WitRelease,
    SearchTerms as WitSearchTerms,
};

/// Per-store data: the resource limiter plus the granted capability.
///
/// wasmtime threads this through every host import and the `Store::limiter`
/// callback. Keeping the capability here is what lets the same generated `Host`
/// impl serve a real fetcher or a deny-all stub with no branching in the guest.
struct HostState {
    limits: StoreLimits,
    http: Arc<dyn HttpCapability>,
}

// The generated `http::Host` trait — the host side of the one granted import.
// A guest calling `fetch` lands here; we defer entirely to the configured
// capability, which enforces allow-list / deny-all policy.
impl HttpHost for HostState {
    fn fetch(&mut self, url: String) -> std::result::Result<WitHttpResponse, WitPluginError> {
        match self.http.fetch(&url) {
            Ok(resp) => Ok(WitHttpResponse {
                status: resp.status,
                body: resp.body,
            }),
            // A denied/failed capability is a *guest-visible* error, not a host
            // trap: the guest gets a `result::err` it can handle.
            Err(e) => Err(WitPluginError {
                message: e.to_string(),
            }),
        }
    }
}

/// A compiled indexer plugin, ready to instantiate per call.
///
/// Compilation (the expensive step) happens once in [`IndexerPluginHost::from_bytes`];
/// each `search`/`latest`/`name` call gets a fresh `Store` and instance.
pub struct IndexerPluginHost {
    engine: Engine,
    component: Component,
    linker: Linker<HostState>,
    config: HostConfig,
    indexer_id: IndexerId,
    /// The granted HTTP capability, if any. `None` (or `grant_http == false`)
    /// means every fetch is denied.
    http_capability: Option<Arc<dyn HttpCapability>>,
    /// Set when the background epoch ticker should stop (on drop).
    epoch_stop: Arc<AtomicBool>,
}

impl IndexerPluginHost {
    /// Compile a component from raw bytes (a `.wasm` component, or WAT text that
    /// wasmtime's `wat` feature accepts) under the given policy.
    ///
    /// `indexer_id` is the id the host stamps onto every [`Release`] the guest
    /// returns — the guest never invents one.
    pub fn from_bytes(bytes: &[u8], config: HostConfig, indexer_id: IndexerId) -> Result<Self> {
        let mut wasm_config = Config::new();
        wasm_config.wasm_component_model(true);
        if config.fuel.is_some() {
            wasm_config.consume_fuel(true);
        }
        if config.epoch_timeout.is_some() {
            wasm_config.epoch_interruption(true);
        }

        let engine = Engine::new(&wasm_config).map_err(|e| PluginError::Load(e.to_string()))?;

        let component =
            Component::new(&engine, bytes).map_err(|e| PluginError::Load(e.to_string()))?;

        // Build the linker with ONLY the http import. No WASI is added: a guest
        // referencing any other interface fails to instantiate.
        let mut linker: Linker<HostState> = Linker::new(&engine);
        cellarr::plugins::http::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |s| s)
            .map_err(|e| PluginError::Host(e.to_string()))?;

        let epoch_stop = Arc::new(AtomicBool::new(false));
        if let Some(timeout) = config.epoch_timeout {
            Self::spawn_epoch_ticker(&engine, timeout, Arc::clone(&epoch_stop));
        }

        Ok(Self {
            engine,
            component,
            linker,
            config,
            indexer_id,
            http_capability: None,
            epoch_stop,
        })
    }

    /// Grant a concrete HTTP capability to this host.
    ///
    /// Only takes effect when [`HostConfig::grant_http`] is `true`; otherwise the
    /// host stays deny-all regardless. This is the explicit grant — there is no
    /// way for a guest to obtain network access except through a capability
    /// installed here.
    #[must_use]
    pub fn with_http_capability(mut self, cap: Arc<dyn HttpCapability>) -> Self {
        self.http_capability = Some(cap);
        self
    }

    /// Spawn a background thread that bumps the engine's epoch once per
    /// `timeout`. Combined with a per-call deadline of 1 tick, this gives each
    /// guest call roughly `timeout` of wall-clock before it traps — catching
    /// tight loops that consume no fuel-measurable progress.
    fn spawn_epoch_ticker(engine: &Engine, timeout: Duration, stop: Arc<AtomicBool>) {
        let engine = engine.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(timeout);
                engine.increment_epoch();
            }
        });
    }

    /// Build a fresh store for one invocation, applying fuel, epoch deadline,
    /// memory limits, and the configured capability.
    fn fresh_store(&self) -> Result<Store<HostState>> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(self.config.max_memory_bytes)
            .tables(self.config.max_tables)
            .table_elements(self.config.max_table_elements)
            .instances(self.config.max_instances)
            .memories(1)
            .build();

        let http: Arc<dyn HttpCapability> = if self.config.grant_http {
            // A granted-but-unset http is still deny-all — the safe direction.
            self.http_capability
                .clone()
                .unwrap_or_else(|| Arc::new(DenyAllHttp))
        } else {
            Arc::new(DenyAllHttp)
        };

        let mut store = Store::new(&self.engine, HostState { limits, http });
        store.limiter(|state| &mut state.limits);

        if let Some(fuel) = self.config.fuel {
            store
                .set_fuel(fuel)
                .map_err(|e| PluginError::Host(e.to_string()))?;
        }
        if self.config.epoch_timeout.is_some() {
            // One tick of headroom; the ticker bumps the epoch every `timeout`.
            store.set_epoch_deadline(1);
        }
        Ok(store)
    }

    /// Instantiate and call the guest's `name()`.
    pub fn name(&self) -> Result<String> {
        let mut store = self.fresh_store()?;
        let instance = self.instantiate(&mut store)?;
        let guest = instance.cellarr_plugins_indexer();
        guest
            .call_name(&mut store)
            .map_err(Self::map_invocation_error)
    }

    /// Instantiate and call the guest's `search()`, adapting to core types.
    pub fn search(&self, terms: &SearchTerms) -> Result<Vec<Release>> {
        let mut store = self.fresh_store()?;
        let instance = self.instantiate(&mut store)?;
        let guest = instance.cellarr_plugins_indexer();
        let wit_terms = to_wit_terms(terms);
        let result = guest
            .call_search(&mut store, &wit_terms)
            .map_err(Self::map_invocation_error)?;
        self.adapt_release_result(result)
    }

    /// Instantiate and call the guest's `latest()`, adapting to core types.
    pub fn latest(&self) -> Result<Vec<Release>> {
        let mut store = self.fresh_store()?;
        let instance = self.instantiate(&mut store)?;
        let guest = instance.cellarr_plugins_indexer();
        let result = guest
            .call_latest(&mut store)
            .map_err(Self::map_invocation_error)?;
        self.adapt_release_result(result)
    }

    fn instantiate(&self, store: &mut Store<HostState>) -> Result<IndexerPlugin> {
        IndexerPlugin::instantiate(store, &self.component, &self.linker)
            .map_err(Self::map_invocation_error)
    }

    fn adapt_release_result(
        &self,
        result: std::result::Result<Vec<WitRelease>, WitPluginError>,
    ) -> Result<Vec<Release>> {
        match result {
            Ok(rs) => Ok(rs
                .into_iter()
                .map(|r| from_wit_release(r, self.indexer_id))
                .collect()),
            Err(e) => Err(PluginError::Guest(e.message)),
        }
    }

    /// Map a wasmtime invocation error: a fuel/epoch/memory trap becomes
    /// [`PluginError::ResourceLimit`], everything else [`PluginError::Host`].
    fn map_invocation_error(e: wasmtime::Error) -> PluginError {
        let msg = format!("{e:#}");
        let lower = msg.to_lowercase();
        if lower.contains("fuel")
            || lower.contains("epoch")
            || lower.contains("interrupt")
            || lower.contains("out of bounds")
            || lower.contains("maximum")
            || lower.contains("grow")
            || lower.contains("limit")
        {
            PluginError::ResourceLimit(msg)
        } else {
            PluginError::Host(msg)
        }
    }
}

impl Drop for IndexerPluginHost {
    fn drop(&mut self) {
        // Stop the epoch ticker so we don't leak the thread.
        self.epoch_stop.store(true, Ordering::Relaxed);
    }
}

fn to_wit_terms(terms: &SearchTerms) -> WitSearchTerms {
    WitSearchTerms {
        queries: terms.queries.clone(),
        ids: terms.ids.clone(),
        numbering: terms.numbering.clone(),
    }
}

fn from_wit_release(r: WitRelease, indexer_id: IndexerId) -> Release {
    Release {
        indexer_id,
        title: r.title,
        download_url: r.download_url,
        guid: r.guid,
        protocol: match r.protocol {
            WitProtocol::Torrent => Protocol::Torrent,
            WitProtocol::Usenet => Protocol::Usenet,
        },
        size: r.size,
        seeders: r.seeders,
        indexer_flags: r.indexer_flags,
    }
}
