//! `locusd` — the local Locus daemon.
//!
//! Scaffolding at U-001. Lifecycle management and cross-platform IPC land in
//! U-006.

use anyhow::Result;

fn main() -> Result<()> {
    locus_core::logging::init();
    println!("locusd {}", locus_core::VERSION);
    Ok(())
}
