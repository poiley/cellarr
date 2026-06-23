//! Host configuration: resource limits and capability grants.
//!
//! [`HostConfig`] is the single place that decides how much a plugin is allowed
//! to do. It is deliberately strict by default — no network, modest CPU/memory
//! budgets — so forgetting to configure a plugin fails closed, not open.
//!
//! The three CPU/memory controls map onto the three wasmtime mechanisms the spec
//! calls for:
//! - **fuel** bounds total executed work (deterministic),
//! - **epoch deadline** bounds wall-clock time (catches tight infinite loops
//!   that a fuel budget alone could let run too long),
//! - **`StoreLimits`** caps linear memory / table growth.

use std::time::Duration;

/// Default fuel budget: enough for a normal parse-and-format pass, small enough
/// that a runaway guest is killed quickly. Callers tune per-plugin.
pub const DEFAULT_FUEL: u64 = 50_000_000;

/// Default linear-memory cap (16 MiB). Generous for an indexer plugin's
/// string/XML work, far below anything that would pressure the daemon.
pub const DEFAULT_MEMORY_BYTES: usize = 16 * 1024 * 1024;

/// Default wall-clock budget per guest call.
pub const DEFAULT_EPOCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Resource and capability policy for a single plugin host.
///
/// Built strict (no capabilities granted) and adjusted with the `with_*`
/// builders. The host reads these to configure the `Engine`, `Store`, and
/// `Linker` before instantiation.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Total fuel granted per guest invocation. `None` disables fuel metering
    /// (only sensible for fully trusted, first-party components).
    pub fuel: Option<u64>,
    /// Maximum linear memory the guest may allocate, in bytes.
    pub max_memory_bytes: usize,
    /// Maximum number of WebAssembly tables / table elements (kept small; an
    /// indexer plugin needs almost none).
    pub max_tables: usize,
    /// Max elements across all tables.
    pub max_table_elements: usize,
    /// Max concurrent instances in the store (1 — fresh instance per call).
    pub max_instances: usize,
    /// Wall-clock budget per guest call, enforced via epoch interruption.
    /// `None` disables the epoch deadline.
    pub epoch_timeout: Option<Duration>,
    /// Whether the guest is granted the HTTP-fetch capability. When `false`
    /// (the default) the host wires a deny-all fetcher, so the guest's import is
    /// present but refuses every call — no ambient authority.
    pub grant_http: bool,
}

impl Default for HostConfig {
    /// The safe default: fuel- and time- and memory-bounded, network denied.
    fn default() -> Self {
        Self {
            fuel: Some(DEFAULT_FUEL),
            max_memory_bytes: DEFAULT_MEMORY_BYTES,
            max_tables: 1,
            max_table_elements: 10_000,
            max_instances: 1,
            epoch_timeout: Some(DEFAULT_EPOCH_TIMEOUT),
            grant_http: false,
        }
    }
}

impl HostConfig {
    /// Grant the narrow HTTP-fetch capability to this plugin. The concrete
    /// fetcher (allow-list, transport) is supplied separately when the host is
    /// built; this flag only decides whether the import is live or deny-all.
    #[must_use]
    pub fn with_http(mut self, grant: bool) -> Self {
        self.grant_http = grant;
        self
    }

    /// Override the fuel budget. `None` disables fuel metering.
    #[must_use]
    pub fn with_fuel(mut self, fuel: Option<u64>) -> Self {
        self.fuel = fuel;
        self
    }

    /// Override the linear-memory cap.
    #[must_use]
    pub fn with_max_memory_bytes(mut self, bytes: usize) -> Self {
        self.max_memory_bytes = bytes;
        self
    }

    /// Override the wall-clock budget. `None` disables the epoch deadline.
    #[must_use]
    pub fn with_epoch_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.epoch_timeout = timeout;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_fail_closed() {
        let cfg = HostConfig::default();
        assert!(!cfg.grant_http, "http must be denied by default");
        assert!(cfg.fuel.is_some(), "fuel metering must be on by default");
        assert!(
            cfg.epoch_timeout.is_some(),
            "epoch deadline must be on by default"
        );
        assert_eq!(cfg.max_instances, 1, "fresh instance per call");
    }

    #[test]
    fn builders_compose() {
        let cfg = HostConfig::default()
            .with_http(true)
            .with_fuel(Some(123))
            .with_max_memory_bytes(4096)
            .with_epoch_timeout(None);
        assert!(cfg.grant_http);
        assert_eq!(cfg.fuel, Some(123));
        assert_eq!(cfg.max_memory_bytes, 4096);
        assert!(cfg.epoch_timeout.is_none());
    }
}
