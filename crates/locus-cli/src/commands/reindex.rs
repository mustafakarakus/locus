use anyhow::Result;
use clap::Parser;
use locus_core::store::Store;

/// Rebuild the search index (consistency repair for FTS5)
#[derive(Parser, Debug)]
pub struct ReindexCmd {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

impl ReindexCmd {
    pub fn run(self) -> Result<()> {
        let store = Store::open_default()?;
        let count = store.reindex()?;

        if self.json {
            println!(
                "{{\"status\":\"ok\",\"reindexed\":{}}}",
                serde_json::json!(count)
            );
        } else {
            println!("✓ Rebuilt search index: {} memory(ies)", count);
        }

        Ok(())
    }
}
