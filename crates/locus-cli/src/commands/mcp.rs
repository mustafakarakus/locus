//! `locus mcp` — start the MCP stdio server for AI coding agents.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use locus_mcp::{run_stdio, Config};

/// Run the Locus MCP server over stdio
#[derive(Parser, Debug)]
pub struct McpCmd {
    /// Override the Locus data directory (default: ~/.locus or $LOCUS_HOME)
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Path to the `locusd` binary used for auto-start
    #[arg(long)]
    pub locusd: Option<PathBuf>,
}

impl McpCmd {
    pub fn run(self) -> Result<()> {
        run_stdio(Config {
            data_dir: self.data_dir,
            locusd_bin: self.locusd,
        })
    }
}
