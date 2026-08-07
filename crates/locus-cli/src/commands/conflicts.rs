use anyhow::Result;
use clap::Parser;
use locus_core::store::Store;

/// List potential memory conflicts
#[derive(Parser, Debug)]
pub struct ConflictsCmd {
    /// Filter conflicts by namespace
    #[arg(short, long)]
    pub namespace: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

impl ConflictsCmd {
    pub fn run(self) -> Result<()> {
        let store = Store::open_default()?;
        let conflicts = store.list_conflicts(self.namespace.clone())?;

        if self.json {
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "status": "ok",
                    "count": conflicts.len(),
                    "conflicts": conflicts,
                }))?
            );
            return Ok(());
        }

        if conflicts.is_empty() {
            println!("No conflicts found.");
            return Ok(());
        }

        println!("Found {} potential conflict(s):\n", conflicts.len());
        for c in &conflicts {
            println!(
                "  [{id}] {a}  <->  {b}",
                id = c.id,
                a = &c.memory_id_a[..c.memory_id_a.len().min(8)],
                b = &c.memory_id_b[..c.memory_id_b.len().min(8)],
            );
            println!("        Reason : {}", c.reason);
            println!("        Detected: {}", format_unix(c.detected_at));
            if let Some(resolved) = c.resolved_at {
                println!("        Resolved: {}", format_unix(resolved));
            }
            println!();
        }

        Ok(())
    }
}

fn format_unix(ts: i64) -> String {
    use time::{format_description::well_known::Rfc3339, OffsetDateTime};
    OffsetDateTime::from_unix_timestamp(ts)
        .ok()
        .and_then(|dt| dt.format(&Rfc3339).ok())
        .unwrap_or_else(|| ts.to_string())
}
