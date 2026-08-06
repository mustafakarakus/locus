//! `locus` — the human-facing CLI.
//!
//! Scaffolding at U-001. Command implementations (`remember`, `search`,
//! `context`, `forget`, `status`, `doctor`, `reindex`) land in U-005.

use anyhow::Result;

fn main() -> Result<()> {
    locus_core::logging::init();
    println!("locus {}", locus_core::VERSION);
    Ok(())
}
