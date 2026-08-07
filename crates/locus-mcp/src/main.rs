//! `locus-mcp` — MCP server bridging AI tools to Locus over stdio.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use locus_mcp::{run_stdio, Config};

/// Locus MCP server (JSON-RPC 2.0 over stdio)
#[derive(Parser, Debug)]
#[command(name = "locus-mcp")]
#[command(version = locus_core::VERSION)]
#[command(about = "MCP server exposing Locus memory tools over stdio", long_about = None)]
struct Args {
    /// Override the Locus data directory (default: ~/.locus or $LOCUS_HOME)
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Path to the `locusd` binary used for auto-start
    #[arg(long)]
    locusd: Option<PathBuf>,

    /// Enable debug logging on stderr
    #[arg(long)]
    debug: bool,
}

fn main() -> Result<()> {
    locus_core::logging::init();

    let args = Args::parse();
    if args.debug {
        // Logging stays on stderr — stdout is reserved for MCP messages.
        std::env::set_var("RUST_LOG", "debug");
        locus_core::logging::init();
    }

    run_stdio(Config {
        data_dir: args.data_dir,
        locusd_bin: args.locusd,
    })
}
