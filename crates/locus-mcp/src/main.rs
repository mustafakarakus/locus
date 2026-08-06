//! `locus-mcp` — the MCP server bridging AI tools to Locus over stdio.
//!
//! Scaffolding at U-001. Tool definitions (`memory_search`, `memory_save`,
//! `memory_forget`, `memory_status`) and stdio framing land in U-007.

use anyhow::Result;

fn main() -> Result<()> {
    locus_core::logging::init();
    println!("locus-mcp {}", locus_core::VERSION);
    Ok(())
}
