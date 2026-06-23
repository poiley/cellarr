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
use clap::{Parser, Subcommand};

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
    /// Print version information.
    Version,
}

#[tokio::main]
async fn main() -> ExitCode {
    match real_main().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // One line per cause so the chain is readable; tracing may not be up
            // yet (config errors happen before logging init), so go to stderr.
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn real_main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref()).context("loading configuration")?;

    match cli.command.unwrap_or(Command::Run) {
        Command::Run => {
            init_tracing(&config.log.filter);
            boot::run(config).await
        }
        Command::Migrate { sources } => {
            init_tracing(&config.log.filter);
            run_migrate(&config, &sources).await
        }
        Command::ConfigCheck => run_config_check(&config),
        Command::Version => {
            println!("cellarr {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

/// Initialize structured logging from the resolved filter, deferring to `RUST_LOG`
/// when set so an operator can override at the process boundary. Idempotent-safe:
/// a second init (e.g. in tests) is ignored rather than panicking.
fn init_tracing(filter: &str) {
    use tracing_subscriber::EnvFilter;
    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(filter))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .try_init();
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
    Ok(())
}
