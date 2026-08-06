use anyhow::Result;
use clap::Parser;
use locus_core::store::Store;

/// Show system and database status
#[derive(Parser, Debug)]
pub struct StatusCmd {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

impl StatusCmd {
    pub fn run(self) -> Result<()> {
        // Try to open the store
        let _store = Store::open_default()?;

        // TODO: Get memory count and other stats
        // For now, just show basic status
        let status = "ok";
        let version = locus_core::VERSION;

        if self.json {
            println!(
                "{{\"status\":\"{}\",\"version\":\"{}\",\"database\":\"ok\"}}",
                status, version
            );
        } else {
            println!("Locus Status");
            println!("  Version: {}", version);
            println!("  Database: ok");
            println!("  Status: {}", status);
        }

        Ok(())
    }
}
