use anyhow::Result;
use clap::Parser;
use locus_core::store::Store;

/// Delete a memory by ID
#[derive(Parser, Debug)]
pub struct ForgetCmd {
    /// Memory ID to delete
    pub id: String,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

impl ForgetCmd {
    pub fn run(self) -> Result<()> {
        // Open the store and delete memory
        let store = Store::open_default()?;
        store.delete_memory(&self.id)?;

        if self.json {
            println!("{{\"status\":\"ok\",\"id\":\"{}\"}}", self.id);
        } else {
            println!("✓ Forgotten memory: {}", self.id);
        }

        Ok(())
    }
}
