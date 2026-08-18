use anyhow::{bail, Result};
use clap::Parser;
use locus_core::ipc::paths::Paths;
use locus_core::ipc::DaemonClient;
use locus_core::store::Store;

/// Delete one memory or wipe all memories
#[derive(Parser, Debug)]
pub struct ForgetCmd {
    /// Memory ID to delete
    pub id: Option<String>,

    /// Delete every stored memory and start fresh
    #[arg(long)]
    pub all: bool,

    /// Confirm the irreversible --all operation
    #[arg(long, requires = "all")]
    pub yes: bool,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

impl ForgetCmd {
    pub fn run(self) -> Result<()> {
        if self.all {
            if self.id.is_some() {
                bail!("provide either a memory ID or --all, not both");
            }
            if !self.yes {
                bail!("refusing to wipe all memories without --yes");
            }

            // The daemon owns the normal single-writer path. Stop and drain it
            // before the destructive reset so no queued operation can restore
            // stale state after the wipe commits.
            let paths = Paths::resolve()?;
            let client = DaemonClient::new(paths.endpoint().clone());
            crate::commands::daemon::stop_if_running(&client)?;
            let store = Store::open_at(paths.db_file())?;
            let deleted = store.delete_all_memories()?;
            if self.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "status": "ok",
                        "deleted": deleted,
                    })
                );
            } else {
                println!("✓ Wiped {deleted} memory(ies). Locus is ready to start fresh.");
            }
            return Ok(());
        }

        let Some(id) = self.id else {
            bail!("provide a memory ID, or use --all --yes to start fresh");
        };
        let store = Store::open_default()?;
        store.delete_memory(&id)?;

        if self.json {
            println!(
                "{}",
                serde_json::json!({
                    "status": "ok",
                    "id": id,
                })
            );
        } else {
            println!("✓ Forgotten memory: {id}");
        }

        Ok(())
    }
}
