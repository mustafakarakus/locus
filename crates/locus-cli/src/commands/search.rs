use anyhow::Result;
use clap::Parser;
use locus_core::memory::MemoryType;
use locus_core::search::Query;
use locus_core::store::Store;

/// Search for memories
#[derive(Parser, Debug)]
pub struct SearchCmd {
    /// Search query
    pub query: String,

    /// Filter by namespace
    #[arg(short, long)]
    pub namespace: Option<String>,

    /// Filter by memory type
    #[arg(short, long)]
    pub r#type: Option<String>,

    /// Maximum number of results
    #[arg(short, long, default_value = "10")]
    pub limit: usize,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

impl SearchCmd {
    pub fn run(self) -> Result<()> {
        // Parse memory type if provided
        let memory_type = match self.r#type.as_deref() {
            Some(t) => Some(MemoryType::parse(t)?),
            None => None,
        };

        // Open the store and search
        let store = Store::open_default()?;
        let query = Query {
            text: self.query.clone(),
            namespace: self.namespace.clone(),
            memory_type,
            limit: self.limit,
        };

        let hits = store.search(query)?;

        if self.json {
            println!(
                "{{\"status\":\"ok\",\"count\":{},\"hits\":{}}}",
                hits.len(),
                serde_json::to_string(&hits)?
            );
        } else {
            if hits.is_empty() {
                println!("No results found for: {}", self.query);
                return Ok(());
            }
            println!(
                "Search results for '{}' ({} found):",
                self.query,
                hits.len()
            );
            for (i, hit) in hits.iter().enumerate() {
                println!(
                    "\n{}. [{}] {} (relevance: {:.2})",
                    i + 1,
                    hit.id,
                    hit.snippet,
                    hit.relevance
                );
            }
        }

        Ok(())
    }
}
