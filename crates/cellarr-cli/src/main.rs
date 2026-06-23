//! cellarr — the daemon and CLI entry point (stub).
//!
//! Runs the cellarr daemon and exposes subcommands (`migrate`, `config check`,
//! `task <name>`, `version`). Not yet implemented; this stub parses args so the
//! binary builds and the workspace resolves. Real work lands per
//! `docs/specs/cellarr-cli.md`.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cellarr",
    version,
    about = "cellarr — unified media acquisition"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Migrate an existing *arr database into cellarr.
    Migrate,
    /// Validate the effective configuration.
    Config,
    /// Run a named task on demand.
    Task { name: String },
    /// Print version information.
    Version,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Version) | None => println!("cellarr {}", env!("CARGO_PKG_VERSION")),
        Some(Command::Migrate) | Some(Command::Config) | Some(Command::Task { .. }) => {
            anyhow::bail!("not yet implemented");
        }
    }
    Ok(())
}
