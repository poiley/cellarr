//! cellarr — the daemon and CLI entry point.
//!
//! The only place wiring happens (`docs/specs/cellarr-cli.md`): parse args and
//! the layered config, then either run the daemon ([`boot`]) or execute an
//! operator subcommand. `anyhow` carries errors at this top level; the libraries
//! underneath use typed `thiserror` errors.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use cellarr_cli::{boot, config::Config};
use cellarr_db::Database;
use clap::{Parser, Subcommand};

mod otel;

#[derive(Parser)]
#[command(
    name = "cellarr",
    version,
    about = "cellarr — unified media acquisition"
)]
struct Cli {
    /// Path to a TOML config file. Optional: with none, built-in defaults plus
    /// `CELLARR_*` environment variables fully configure the daemon (zero-config
    /// startup).
    #[arg(long, short = 'c', global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon (the default when no subcommand is given).
    Run,
    /// Import an existing Radarr/Sonarr SQLite database into the cellarr DB.
    Migrate {
        /// Path(s) to the source *arr database file(s). A Radarr and a Sonarr DB
        /// can be imported together into one unified library set.
        #[arg(required = true, value_name = "SOURCE_DB")]
        sources: Vec<PathBuf>,
    },
    /// Validate the effective (layered) configuration and print it.
    #[command(name = "config")]
    ConfigCheck,
    /// Managed-config (config-as-code) operations: validate a declarative file
    /// against the live DB, or export the current DB state as a file.
    #[command(subcommand)]
    ManagedConfig(ManagedConfigCommand),
    /// Print version information.
    Version,
}

/// The `cellarr managed-config` subcommand group (config-as-code).
#[derive(Subcommand)]
enum ManagedConfigCommand {
    /// Load + interpolate + validate a managed-config file and compute its plan
    /// against the configured DB, printing a human diff.
    ///
    /// Exit codes: `0` clean (no drift), `2` validation/load error, `3` valid but
    /// pending drift (the file would change the DB). The distinct drift code lets
    /// CI gate on "the live config matches git".
    Validate {
        /// The managed-config file. Defaults to the configured `managed_config_path`.
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
    },
    /// Dump the current DB state of the managed-able kinds as a managed-config
    /// YAML document (round-trippable; secrets emitted as `${ENV}` placeholders).
    Export {
        /// Write the YAML here instead of stdout.
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
    },
}

/// The exit code `config validate` returns when the file is valid but the DB has
/// pending drift (a reconcile would change something). Distinct from a hard error.
const EXIT_DRIFT: u8 = 3;
/// The exit code `config validate` returns on a load/validation error.
const EXIT_CONFIG_ERROR: u8 = 2;

#[tokio::main]
async fn main() -> ExitCode {
    match real_main().await {
        Ok(code) => code,
        Err(e) => {
            // One line per cause so the chain is readable; tracing may not be up
            // yet (config errors happen before logging init), so go to stderr.
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn real_main() -> Result<ExitCode> {
    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref()).context("loading configuration")?;

    match cli.command.unwrap_or(Command::Run) {
        Command::Run => {
            // Hold the guards for the whole daemon lifetime so the non-blocking
            // writer and the OTLP exporter flush their buffers on shutdown.
            let _log_guard = init_tracing(
                &config.log.filter,
                Some(&config.log_dir()),
                config.otel.endpoint.as_deref(),
            );
            boot::run(config).await.map(|()| ExitCode::SUCCESS)
        }
        Command::Migrate { sources } => {
            let _log_guard = init_tracing(
                &config.log.filter,
                Some(&config.log_dir()),
                config.otel.endpoint.as_deref(),
            );
            run_migrate(&config, &sources)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Command::ConfigCheck => run_config_check(&config).map(|()| ExitCode::SUCCESS),
        Command::ManagedConfig(cmd) => run_managed_config(&config, cmd).await,
        Command::Version => {
            println!("cellarr {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Initialize structured logging from the resolved filter, deferring to `RUST_LOG`
/// when set so an operator can override at the process boundary.
///
/// When `log_dir` is given, a **rolling daily file appender** is added beside the
/// console layer, writing to `<log_dir>/cellarr.log` (rotated daily). This is the
/// file the `/api/v3/log/file` surface reads back. The returned
/// [`tracing_appender::non_blocking::WorkerGuard`] must be held for the process
/// lifetime so the background writer flushes on shutdown; dropping it early loses
/// buffered lines.
///
/// Idempotent-safe: a second init (e.g. in tests) is ignored rather than
/// panicking, in which case no guard is returned.
/// Guards that must outlive the process: the non-blocking file appender's writer
/// and the OTLP exporter. Dropping either flushes its buffered output, so the
/// returned value is held for the daemon's whole lifetime.
#[derive(Default)]
#[must_use]
struct TracingGuards {
    _appender: Option<tracing_appender::non_blocking::WorkerGuard>,
    _otel: Option<otel::OtelGuard>,
}

#[must_use]
fn init_tracing(
    filter: &str,
    log_dir: Option<&std::path::Path>,
    otel_endpoint: Option<&str>,
) -> TracingGuards {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Layer};

    let make_filter = || {
        EnvFilter::try_from_default_env()
            .or_else(|_| EnvFilter::try_new(filter))
            .unwrap_or_else(|_| EnvFilter::new("info"))
    };

    // Every sink is a boxed layer on the root registry so the set is composed the
    // same way whether or not the file appender and OTLP exporter are present.
    let mut layers: Vec<Box<dyn Layer<tracing_subscriber::Registry> + Send + Sync>> =
        vec![tracing_subscriber::fmt::layer().boxed()];
    let mut guards = TracingGuards::default();

    // Rolling daily file appender. If its directory cannot be created we degrade
    // to console-only rather than refusing to start (logging must never block the
    // daemon from running).
    if let Some(dir) = log_dir {
        match std::fs::create_dir_all(dir) {
            Ok(()) => {
                let appender = tracing_appender::rolling::daily(dir, "cellarr.log");
                let (writer, guard) = tracing_appender::non_blocking(appender);
                layers.push(
                    tracing_subscriber::fmt::layer()
                        .with_writer(writer)
                        .with_ansi(false)
                        .boxed(),
                );
                guards._appender = Some(guard);
            }
            Err(e) => eprintln!("warning: could not create log dir {}: {e}", dir.display()),
        }
    }

    // Opt-in OTLP export: only when an endpoint is configured (and, at build time,
    // the `otlp` feature is on — otherwise `otlp_layer` is a no-op).
    if let Some(endpoint) = otel_endpoint.filter(|e| !e.is_empty()) {
        if let Some((layer, guard)) = otel::otlp_layer(endpoint) {
            layers.push(layer);
            guards._otel = Some(guard);
        }
    }

    // The EnvFilter is added last as a global filter over every layer above. A
    // second init (e.g. in tests) is ignored rather than panicking, in which case
    // the guards are dropped immediately — there is nothing to flush.
    let registered = tracing_subscriber::registry()
        .with(layers)
        .with(make_filter())
        .try_init()
        .is_ok();
    if registered {
        guards
    } else {
        TracingGuards::default()
    }
}

/// Drive `cellarr-migrate` end to end: import the source database(s) into a fresh
/// cellarr database under the configured data dir.
async fn run_migrate(config: &Config, sources: &[PathBuf]) -> Result<()> {
    use cellarr_db::Database;

    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("creating data dir {}", config.data_dir.display()))?;
    let db_path = config.database_path();
    let db = Database::open(
        db_path
            .to_str()
            .context("database path is not valid UTF-8")?,
    )
    .await
    .with_context(|| format!("opening destination database at {}", db_path.display()))?;

    let source_strs: Vec<&str> = sources
        .iter()
        .map(|p| p.to_str().context("source path is not valid UTF-8"))
        .collect::<Result<_>>()?;

    let report = cellarr_migrate::import(&source_strs, &db)
        .await
        .context("importing source database(s)")?;

    // Stop the writer-actor and close the pool so the import is durable on disk
    // before we return. (Closing the pool directly would deadlock on the actor's
    // held connection — `shutdown` stops it first.)
    db.shutdown().await;

    let p = &report.preview;
    println!(
        "imported {} libraries, {} content nodes ({} items), {} files, {} identities; \
         {} profiles, {} custom formats, {} indexers, {} download clients ({} file operations)",
        p.library_count,
        p.content_count,
        p.item_count,
        p.file_count,
        report.identities_written,
        p.profile_count,
        p.custom_format_count,
        p.indexer_count,
        p.download_client_count,
        p.scheduled_file_operations,
    );
    Ok(())
}

/// Validate and print the effective configuration. Loading already happened
/// (any error surfaced before this), so reaching here means the config is valid.
fn run_config_check(config: &Config) -> Result<()> {
    println!("configuration OK");
    println!("  data_dir   = {}", config.data_dir.display());
    println!("  database   = {}", config.database_path().display());
    println!("  api.bind   = {}:{}", config.api.bind, config.api.port);
    println!(
        "  api.auth   = {}",
        if config.api.api_key.is_some() {
            "enabled"
        } else {
            "disabled (no key set)"
        }
    );
    println!("  log.filter = {}", config.log.filter);
    println!("  metrics    = {}", config.metrics.enabled);
    println!(
        "  managed    = {}",
        config
            .managed_config_path
            .as_ref()
            .map_or_else(|| "none".to_string(), |p| p.display().to_string())
    );
    Ok(())
}

/// Drive the `cellarr managed-config` subcommand group (config-as-code).
///
/// `validate` loads + interpolates + validates the file, computes the reconcile
/// plan against the live DB (read-only), prints the diff, and exits with a
/// distinct code on drift. `export` dumps the current DB state as a managed-config
/// YAML document. Both open the configured database under the data dir.
async fn run_managed_config(config: &Config, cmd: ManagedConfigCommand) -> Result<ExitCode> {
    use cellarr_cli::managed;

    // Resolve the target file: the explicit `--file`, else the configured path.
    let resolve_file = |explicit: Option<PathBuf>| -> Result<PathBuf> {
        explicit
            .or_else(|| config.managed_config_path.clone())
            .context(
                "no managed-config file given: pass --file PATH or set \
                 CELLARR_MANAGED_CONFIG_PATH / config `managed_config_path`",
            )
    };

    // Open the configured DB (migrations run on open).
    let open_db = || async {
        std::fs::create_dir_all(&config.data_dir)
            .with_context(|| format!("creating data dir {}", config.data_dir.display()))?;
        let db_path = config.database_path();
        let db = Database::open(
            db_path
                .to_str()
                .context("database path is not valid UTF-8")?,
        )
        .await
        .with_context(|| format!("opening database at {}", db_path.display()))?;
        anyhow::Ok(db)
    };

    match cmd {
        ManagedConfigCommand::Validate { file } => {
            let path = resolve_file(file)?;
            // Load + validate. A load/validation error is the config-error exit.
            let managed_config = match managed::loader::load(&path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("managed config invalid: {e}");
                    return Ok(ExitCode::from(EXIT_CONFIG_ERROR));
                }
            };
            let db = open_db().await?;
            let report = managed::reconcile::plan(&db, &managed_config).await?;
            print!("{}", managed::render_diff(&report));
            db.shutdown().await;
            if report.has_changes() {
                println!("pending drift: the live config does not match the file");
                Ok(ExitCode::from(EXIT_DRIFT))
            } else {
                println!("clean: the live config matches the file");
                Ok(ExitCode::SUCCESS)
            }
        }
        ManagedConfigCommand::Export { file } => {
            let db = open_db().await?;
            let exported = managed::export::export(&db).await?;
            db.shutdown().await;
            let yaml = managed::export::to_yaml(&exported)?;
            match file {
                Some(path) => {
                    std::fs::write(&path, yaml)
                        .with_context(|| format!("writing export to {}", path.display()))?;
                    println!("exported managed config to {}", path.display());
                }
                None => print!("{yaml}"),
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
