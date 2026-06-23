//! Typed errors for the plugin host.
//!
//! `cellarr-plugins` is a library, so it follows the convention of typed
//! `thiserror` errors rather than `anyhow`. Each variant distinguishes a class
//! of failure a caller may want to react to differently: a load/compile failure
//! is the author's bug, a *trap* (fuel/epoch/memory) is a resource-policy kill
//! the host must isolate, and a `Guest` error is the plugin reporting a normal
//! failure through its WIT `result`.

/// Errors the WASM plugin host can produce.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginError {
    /// The component bytes could not be compiled or instantiated.
    #[error("failed to load plugin component: {0}")]
    Load(String),

    /// A capability the guest tried to use was not granted by the host.
    ///
    /// This is the "no ambient authority" boundary: a guest reaching for a
    /// capability it was not explicitly given fails here rather than escaping
    /// the sandbox.
    #[error("plugin denied capability: {0}")]
    CapabilityDenied(String),

    /// The guest was terminated by a resource limit — fuel exhaustion, epoch
    /// deadline (wall-clock), or a `StoreLimits` memory/table cap.
    ///
    /// The daemon treats this as "the plugin misbehaved"; the host instance is
    /// dropped and the rest of cellarr is unaffected.
    #[error("plugin exceeded resource limits: {0}")]
    ResourceLimit(String),

    /// The guest ran but reported a domain failure through its WIT `result`.
    #[error("plugin reported an error: {0}")]
    Guest(String),

    /// A failure inside the host while invoking the guest that is not itself a
    /// resource kill (e.g. a type mismatch wiring the linker).
    #[error("plugin host error: {0}")]
    Host(String),
}

/// Convenience alias for fallible host operations.
pub type Result<T> = std::result::Result<T, PluginError>;
