use anyhow::Result;
use clap::Parser;
use locus_core::memory::{MemoryType, NewMemory};
use locus_core::store::Store;

/// Remember a fact, decision, preference, or other memory
#[derive(Parser, Debug)]
pub struct RememberCmd {
    /// The memory content
    pub content: String,

    /// Memory type (fact, decision, preference, task, bug, architecture, code, note)
    #[arg(short, long)]
    pub r#type: Option<String>,

    /// Namespace (default: "global")
    #[arg(short, long)]
    pub namespace: Option<String>,

    /// Importance level (0-100, default: 50)
    #[arg(short, long)]
    pub importance: Option<u8>,

    /// Memory title (optional, defaults to first line of content)
    #[arg(long)]
    pub title: Option<String>,

    /// Entities associated with the memory (space-separated)
    #[arg(long)]
    pub entities: Option<String>,

    /// Source of the memory (optional)
    #[arg(short, long)]
    pub source: Option<String>,

    /// Store detected secrets verbatim instead of redacting them (explicit consent)
    #[arg(long)]
    pub allow_secret: bool,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

impl RememberCmd {
    pub fn run(self) -> Result<()> {
        // Parse memory type
        let memory_type = match self.r#type.as_deref() {
            Some(t) => MemoryType::parse(t)?,
            None => MemoryType::Fact,
        };

        // Validate and use importance
        let importance = self.importance.unwrap_or(50);

        // Parse entities
        let entities = self
            .entities
            .map(|e| e.split_whitespace().map(|s| s.to_string()).collect())
            .unwrap_or_default();

        // Determine title
        let title = self.title.unwrap_or_else(|| {
            self.content
                .lines()
                .next()
                .unwrap_or("Untitled")
                .to_string()
        });

        // Open the store and insert memory
        let store = Store::open_default()?;
        let new_memory = NewMemory {
            namespace: self.namespace,
            memory_type,
            title: title.clone(),
            content: self.content.clone(),
            entities,
            importance,
            source: self.source,
        };

        let (id, warnings) = store.insert_memory_checked(new_memory, self.allow_secret)?;

        if self.json {
            println!(
                "{{\"status\":\"ok\",\"id\":\"{}\",\"title\":\"{}\"}}",
                id, title
            );
        } else {
            println!("✓ Remembered: {}", title);
            println!("  ID: {}", id);
        }

        for warning in &warnings {
            eprintln!("warning: {}", warning.message);
        }

        Ok(())
    }
}
