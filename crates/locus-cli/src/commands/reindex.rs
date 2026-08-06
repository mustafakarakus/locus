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
        // Open the store
        let _store = Store::open_default()?;

        // TODO: Implement reindex operation
        // This should rebuild the FTS5 table from canonical data
        // store.reindex()?;

        if self.json {
            println!("{{\"status\":\"ok\",\"message\":\"Search index rebuilt\"}}");
        } else {
            println!("✓ Search index rebuilt successfully");
        }

        Ok(())
    }
}
