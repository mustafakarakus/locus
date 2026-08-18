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
        let store = Store::open_default()?;

        let database = "ok";
        let memory_count = store.memory_count().unwrap_or(0);
        let fts_row_count = store.fts_row_count().unwrap_or(0);
        let index_ok = if store.fts_out_of_sync().unwrap_or(true) {
            "out-of-sync"
        } else {
            "ok"
        };
        let status = if database == "ok" && index_ok == "ok" {
            "ok"
        } else {
            "degraded"
        };
        let version = locus_core::VERSION;

        if self.json {
            println!(
                "{}",
                serde_json::json!({
                    "status": status,
                    "version": version,
                    "database": database,
                    "memory_count": memory_count,
                    "fts_row_count": fts_row_count,
                    "search_index": index_ok,
                })
            );
        } else {
            println!("Locus Status");
            println!("  Version: {}", version);
            println!("  Database: {}", database);
            println!("  Memories: {}", memory_count);
            println!("  Search index rows: {} ({})", fts_row_count, index_ok);
            println!("  Status: {}", status);
        }

        Ok(())
    }
}
