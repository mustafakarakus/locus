//! `locus` — the human-facing CLI.
//!
//! Scaffolding at U-001. Command implementations (`remember`, `search`,
//! `context`, `forget`, `status`, `doctor`, `reindex`) land in U-005.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;
#[cfg(test)]
mod tests;

use commands::{
    context::ContextCmd, doctor::DoctorCmd, forget::ForgetCmd, reindex::ReindexCmd,
    remember::RememberCmd, search::SearchCmd, status::StatusCmd,
};

/// Locus — local-first, long-term memory for AI coding agents
#[derive(Parser, Debug)]
#[command(name = "locus")]
#[command(version = locus_core::VERSION)]
#[command(about = "Local-first, long-term memory for AI coding agents", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable debug logging
    #[arg(global = true, long)]
    debug: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Remember a fact, decision, preference, or other memory
    Remember(RememberCmd),

    /// Search for memories
    Search(SearchCmd),

    /// Get a compressed context brief
    Context(ContextCmd),

    /// Delete a memory by ID
    Forget(ForgetCmd),

    /// Show system and database status
    Status(StatusCmd),

    /// Diagnose and repair Locus installation
    Doctor(DoctorCmd),

    /// Rebuild the search index (consistency repair for FTS5)
    Reindex(ReindexCmd),
}

fn main() -> Result<()> {
    locus_core::logging::init();

    let cli = Cli::parse();

    if cli.debug {
        std::env::set_var("RUST_LOG", "debug");
        locus_core::logging::init();
    }

    match cli.command {
        Commands::Remember(cmd) => cmd.run(),
        Commands::Search(cmd) => cmd.run(),
        Commands::Context(cmd) => cmd.run(),
        Commands::Forget(cmd) => cmd.run(),
        Commands::Status(cmd) => cmd.run(),
        Commands::Doctor(cmd) => cmd.run(),
        Commands::Reindex(cmd) => cmd.run(),
    }
}
