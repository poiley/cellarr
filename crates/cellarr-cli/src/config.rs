//! The layered daemon configuration.
//!
//! Precedence is built-in defaults → config file → environment (figment merges
//! later providers over earlier ones), which is the order `docs/01-architecture.md`
//! mandates. The whole point is **zero-config startup**: with no file and no env
//! the defaults alone produce a runnable daemon (SQLite under a data dir, the API
//! on `127.0.0.1` at a fixed port), satisfying the single-binary/offline
//! non-negotiable.
//!
//! Secrets (indexer/client/metadata keys) are **not** config — they live in the
//! database, encrypted at rest (`docs/01-architecture.md`); only operational
//! wiring (paths, bind address, toggles) lives here.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use serde::{Deserialize, Serialize};

/// The environment-variable prefix for every config override (e.g.
/// `CELLARR_API__PORT`). The double underscore is figment's nesting separator,
/// so `CELLARR_API__BIND` sets `api.bind`.
const ENV_PREFIX: &str = "CELLARR_";

/// The effective daemon configuration after layering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where the daemon keeps its state (the SQLite file, future downloads
    /// workspace). Defaults to a per-user data dir; everything else is derived
    /// from it unless explicitly overridden.
    pub data_dir: PathBuf,
    /// Optional database connection URL, from `CELLARR_DATABASE_URL`. Unset (the
    /// default) uses the SQLite file under `data_dir` — the zero-config, offline
    /// default. Set to a `postgres://…` DSN to run against a Postgres **server**
    /// instead (only honoured by a build compiled with the `postgres` feature);
    /// this is the deployment escape hatch for when the SQLite file would
    /// otherwise sit on high-latency network storage. A `sqlite://<path>` value
    /// is also accepted to point the SQLite backend at an explicit file.
    pub database_url: Option<String>,
    /// HTTP API server settings.
    pub api: ApiConfig,
    /// Logging settings.
    pub log: LogConfig,
    /// OpenTelemetry (OTLP) export settings — **off by default** (an unset
    /// endpoint means no export; the network exporter is also behind the `otlp`
    /// build feature, so a default build never dials out).
    pub otel: OtelConfig,
    /// Metrics settings — **off by default** (never required, per the spec).
    pub metrics: MetricsConfig,
    /// TheTVDB (TV metadata) credentials. Populated from `CELLARR_TVDB__*`
    /// (typically the gitignored `.env`); absent keys leave the source
    /// unavailable and the daemon degrades gracefully.
    pub tvdb: TvdbConfig,
    /// TMDb (movie metadata) credentials, from `CELLARR_TMDB__*`.
    pub tmdb: TmdbConfig,
    /// Media-management file-handling settings (the recycle-bin path a delete
    /// moves removed media into instead of unlinking).
    pub media_management: MediaManagementConfig,
    /// Optional path to a declarative **managed-config** file (config-as-code).
    /// When set, the daemon reconciles its DB from this file on boot (after
    /// migrations, before serving): tags/root folders/libraries/quality
    /// definitions/custom formats/quality profiles/indexers/download clients are
    /// brought to match the file, with config-managed entities pruned when no
    /// longer declared and UI-created ones left untouched. Unset (the default)
    /// leaves behaviour unchanged — zero-config startup still works. From
    /// `CELLARR_MANAGED_CONFIG_PATH` (or the config file's `managed_config_path`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_config_path: Option<PathBuf>,
}

/// Media-management file-handling configuration.
///
/// The operational mirror of the *arr "Media Management" section: file-handling
/// toggles that change how cellarr touches the library on disk. Only the small
/// set cellarr's file ops reason about lives here; the cosmetic naming options
/// stay in the `/api/v3` projection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaManagementConfig {
    /// The recycle-bin directory a `deleteFiles` content delete moves removed
    /// media into (preserving its layout relative to the library root), making a
    /// mistaken delete reversible. `None`/unset unlinks the files outright (the
    /// *arr default). From `CELLARR_MEDIA_MANAGEMENT__RECYCLE_BIN_PATH`.
    pub recycle_bin_path: Option<PathBuf>,
    /// **Opt-in auto-onboard.** When set, the library rescan does not just adopt
    /// files matching an existing content node — for a file it cannot place, it
    /// looks the parsed title up in the metadata source and, on a HIGH-confidence
    /// match (exact normalized title, and year if the file parsed one), CREATES the
    /// movie/series and adopts the file onto it. Off by default: it creates content
    /// from filenames, so a mis-parse could add a wrong title; leave it off to keep
    /// the conservative "adopt to existing nodes only, surface the rest" behavior.
    /// From `CELLARR_MEDIA_MANAGEMENT__AUTO_ONBOARD`.
    pub auto_onboard: bool,
    /// Cap on how many nodes ONE auto-onboard pass creates. `None` (the default) is
    /// unbounded. Set a small value to onboard a first batch of a large library and
    /// verify the results before lifting the cap for the rest — a rescan is
    /// idempotent, so already-onboarded files are skipped on the next run and it
    /// picks up where it left off. From `CELLARR_MEDIA_MANAGEMENT__AUTO_ONBOARD_LIMIT`.
    pub auto_onboard_limit: Option<usize>,
}

/// HTTP API server settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// The bind address. Defaults to loopback so a fresh install is not exposed
    /// to the network without an explicit choice.
    pub bind: IpAddr,
    /// The TCP port. `0` lets the OS pick (used by tests); the default is a
    /// fixed, documented port for a real daemon.
    pub port: u16,
    /// The API key required on mutating endpoints. `None` (the default) leaves
    /// auth disabled for a single-user local first run; setting it enforces auth.
    pub api_key: Option<String>,
}

/// Logging settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// The tracing filter directive (e.g. `info`, `cellarr=debug`). Overridable
    /// by `CELLARR_LOG__FILTER` or the standard `RUST_LOG`.
    pub filter: String,
}

/// OpenTelemetry (OTLP) export settings.
///
/// Export is opt-in on two axes, both of which must be satisfied for a span to
/// leave the process: the binary must be built with the `otlp` feature, and
/// `endpoint` must be set (from `CELLARR_OTEL__ENDPOINT` or the config file). An
/// unset endpoint — the default — means logging stays entirely local. See
/// [`docs/18-observability.md`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OtelConfig {
    /// The OTLP/HTTP collector endpoint (e.g. `http://localhost:4318`). `None`
    /// (the default) disables export. Only consulted when the `otlp` feature is
    /// compiled in.
    pub endpoint: Option<String>,
}

/// Optional metrics settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricsConfig {
    /// Whether to expose a metrics endpoint. Off by default — metrics are never
    /// required for the daemon to run.
    pub enabled: bool,
}

/// TheTVDB v4 credentials (bring-your-own-key, user-supported model).
///
/// The api key and optional subscriber pin reach this struct from the
/// `CELLARR_TVDB__API_KEY` / `CELLARR_TVDB__PIN` env vars (loaded from `.env` at
/// startup). They are deliberately *not* surfaced in [`Debug`] output so a config
/// dump never leaks the key.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TvdbConfig {
    /// The TheTVDB v4 api key. `None` → the source is unavailable; the daemon
    /// still boots (offline is non-negotiable).
    pub api_key: Option<String>,
    /// Optional subscriber PIN required by the user-supported key model.
    pub pin: Option<String>,
}

impl std::fmt::Debug for TvdbConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the key or pin — only whether each is present.
        f.debug_struct("TvdbConfig")
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("pin", &self.pin.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// TMDb credentials (bring-your-own-key).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TmdbConfig {
    /// The TMDb v4 read token or v3 api key. `None` → source unavailable.
    pub api_key: Option<String>,
}

impl std::fmt::Debug for TmdbConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TmdbConfig")
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            database_url: None,
            api: ApiConfig::default(),
            log: LogConfig::default(),
            otel: OtelConfig::default(),
            metrics: MetricsConfig::default(),
            tvdb: TvdbConfig::default(),
            tmdb: TmdbConfig::default(),
            media_management: MediaManagementConfig::default(),
            managed_config_path: None,
        }
    }
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            // Loopback by default: a zero-config install must not silently bind a
            // public interface.
            bind: IpAddr::from([127, 0, 0, 1]),
            port: DEFAULT_API_PORT,
            api_key: None,
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            filter: "info".to_string(),
        }
    }
}

/// The default API port. Distinct from the *arr ecosystem's 7878/8989 so cellarr
/// can run beside an install being migrated from.
pub const DEFAULT_API_PORT: u16 = 9494;

impl Config {
    /// Load the layered configuration: defaults → optional `config_path` → env.
    ///
    /// A missing config file is **not** an error — zero-config startup means the
    /// defaults stand alone. The env layer reads `CELLARR_*` (nested with `__`,
    /// e.g. `CELLARR_API__PORT=0`).
    ///
    /// # Errors
    /// Returns the figment error if a present config file is malformed or a value
    /// fails to deserialize into [`Config`]. Boxed because `figment::Error` is
    /// large and this sits on a `Result` the binary propagates.
    pub fn load(config_path: Option<&Path>) -> Result<Self, Box<figment::Error>> {
        load_dotenv();
        Self::figment(config_path).extract().map_err(Box::new)
    }

    /// Build the figment used by [`Config::load`]. Exposed so `config check` can
    /// report provider metadata, and so tests can drive layering with explicit
    /// providers.
    #[must_use]
    pub fn figment(config_path: Option<&Path>) -> Figment {
        let mut fig = Figment::from(Serialized::defaults(Config::default()));
        if let Some(path) = config_path {
            // `Toml::file` is a no-op when the file is absent, which is exactly
            // the zero-config behavior we want.
            fig = fig.merge(Toml::file(path));
        }
        fig.merge(Env::prefixed(ENV_PREFIX).split("__"))
    }

    /// The resolved path to the SQLite database file under the data dir.
    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("cellarr.sqlite")
    }

    /// The database connection target the daemon opens: the configured
    /// [`database_url`](Self::database_url) when set, else a `sqlite://` URL for
    /// the file under [`data_dir`](Self::data_dir). This is what boot passes to
    /// `Database::connect`, so the same code opens SQLite or Postgres by config.
    #[must_use]
    pub fn database_target(&self) -> String {
        self.database_url
            .clone()
            .unwrap_or_else(|| format!("sqlite://{}", self.database_path().display()))
    }

    /// The database target with any credentials redacted, safe to log. A
    /// `postgres://<user>:<pw>@<host>/<db>` DSN is reduced to its host/db; a
    /// `sqlite://` target (no secrets) is returned as-is.
    #[must_use]
    pub fn database_target_redacted(&self) -> String {
        let target = self.database_target();
        // Strip an `user[:pass]@` userinfo section from the authority, if any.
        match target.split_once("://") {
            Some((scheme, rest)) => match rest.split_once('@') {
                Some((_userinfo, host_and_path)) => format!("{scheme}://{host_and_path}"),
                None => target,
            },
            None => target,
        }
    }

    /// The directory the rolling log appender writes to (`<data_dir>/logs`), which
    /// the `/api/v3/log/file` surface reads back.
    #[must_use]
    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    /// The directory database backups are written to (`<data_dir>/backups`), which
    /// the `/api/v3/system/backup` surface lists/serves/restores from.
    #[must_use]
    pub fn backup_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }

    /// Build the runtime [`cellarr_meta::TheTvdbConfig`] from the loaded config,
    /// carrying the api key and optional pin onto the source's defaults (base
    /// url, cache TTL, conservative rate limit).
    #[must_use]
    pub fn thetvdb_source_config(&self) -> cellarr_meta::TheTvdbConfig {
        cellarr_meta::TheTvdbConfig {
            api_key: self.tvdb.api_key.clone(),
            pin: self.tvdb.pin.clone(),
            ..cellarr_meta::TheTvdbConfig::default()
        }
    }

    /// Build the runtime [`cellarr_meta::TmdbConfig`] from the loaded config.
    #[must_use]
    pub fn tmdb_source_config(&self) -> cellarr_meta::TmdbConfig {
        cellarr_meta::TmdbConfig {
            api_key: self.tmdb.api_key.clone(),
            ..cellarr_meta::TmdbConfig::default()
        }
    }
}

/// Load a `.env` file into the process environment **before** figment reads the
/// `CELLARR_*` env layer, so the gitignored `.env` (where bring-your-own-key
/// secrets live) reaches config without exporting them in the shell.
///
/// Missing `.env` is not an error (zero-config startup); existing process env
/// always wins over the file so an explicit export can override. The key value
/// is never logged.
fn load_dotenv() {
    match dotenvy::dotenv() {
        Ok(path) => tracing::debug!(path = %path.display(), "loaded .env"),
        Err(e) if e.not_found() => {}
        Err(e) => tracing::warn!(error = %e, "failed to load .env"),
    }
}

/// The default per-user data directory.
///
/// Honors `XDG_DATA_HOME`/`HOME` without pulling a directories crate (keeping the
/// default build lean); falls back to a local `./data` when neither is set so the
/// daemon still has somewhere to write rather than failing to boot.
fn default_data_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(xdg).join("cellarr");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(home).join(".local/share/cellarr");
    }
    PathBuf::from("./data")
}
