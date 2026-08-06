use anyhow::Result;
use clap::Parser;
use locus_core::context::ContextBriefOptions;
use locus_core::search::Query;
use locus_core::store::Store;

/// Get a compressed context brief
#[derive(Parser, Debug)]
pub struct ContextCmd {
    /// Search query to find relevant memories
    pub query: String,

    /// Filter by namespace
    #[arg(short, long)]
    pub namespace: Option<String>,

    /// Maximum number of memories to include
    #[arg(short, long, default_value = "5")]
    pub limit: usize,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

impl ContextCmd {
    pub fn run(self) -> Result<()> {
        // Open the store
        let store = Store::open_default()?;
        let query = Query {
            text: self.query.clone(),
            namespace: self.namespace.clone(),
            memory_type: None,
            limit: self.limit,
        };

        // Generate context brief
        let options = ContextBriefOptions::default();
        let brief = store.context_brief(query, options)?;

        if self.json {
            println!(
                "{{\"status\":\"ok\",\"brief\":{}}}",
                serde_json::to_string(&brief)?
            );
        } else {
            println!("{}", brief);
        }

        Ok(())
    }
}
