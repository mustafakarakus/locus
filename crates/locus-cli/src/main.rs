//! `locus` — the human-facing CLI.
//!
//! Commands: `init`, `remember`, `search`, `context`, `forget`, `status`,
//! `doctor`, `reindex`, `daemon`, `mcp`, `conflicts`, `graph`, `hook`.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;
#[cfg(test)]
mod tests;

use commands::{
    bench::BenchCmd, conflicts::ConflictsCmd, context::ContextCmd, daemon::DaemonCmd,
    doctor::DoctorCmd, forget::ForgetCmd, graph::GraphCmd, hook::HookCmd, init::InitCmd,
    mcp::McpCmd, reindex::ReindexCmd, remember::RememberCmd, search::SearchCmd, status::StatusCmd,
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
    /// Install Locus memory protocol into project rules and MCP config
    Init(InitCmd),

    /// Remember a fact, decision, preference, or other memory
    Remember(RememberCmd),

    /// Search for memories
    Search(SearchCmd),

    /// Get a compressed context brief
    Context(ContextCmd),

    /// Delete a memory by ID
    Forget(ForgetCmd),

    /// Manage git hook based memory ingestion
    Hook(HookCmd),

    /// Show system and database status
    Status(StatusCmd),

    /// Diagnose and repair Locus installation
    Doctor(DoctorCmd),

    /// Rebuild the search index (consistency repair for FTS5)
    Reindex(ReindexCmd),

    /// Control the background Locus daemon (`locusd`)
    Daemon(DaemonCmd),

    /// Run the MCP server over stdio (for AI coding agents)
    Mcp(McpCmd),

    /// List memories that may conflict with each other
    Conflicts(ConflictsCmd),

    /// Render the memory graph as HTML (snapshot or live)
    Graph(GraphCmd),

    /// Run performance benchmarks and check the budget (U-012)
    Bench(BenchCmd),
}

fn main() -> Result<()> {
    locus_core::logging::init();

    let cli = Cli::parse();

    if cli.debug {
        std::env::set_var("RUST_LOG", "debug");
        locus_core::logging::init();
    }

    match cli.command {
        Commands::Init(cmd) => cmd.run(),
        Commands::Remember(cmd) => cmd.run(),
        Commands::Search(cmd) => cmd.run(),
        Commands::Context(cmd) => cmd.run(),
        Commands::Forget(cmd) => cmd.run(),
        Commands::Hook(cmd) => cmd.run(),
        Commands::Status(cmd) => cmd.run(),
        Commands::Doctor(cmd) => cmd.run(),
        Commands::Reindex(cmd) => cmd.run(),
        Commands::Daemon(cmd) => cmd.run(),
        Commands::Mcp(cmd) => cmd.run(),
        Commands::Conflicts(cmd) => cmd.run(),
        Commands::Graph(cmd) => cmd.run(),
        Commands::Bench(cmd) => cmd.run(),
    }
}
