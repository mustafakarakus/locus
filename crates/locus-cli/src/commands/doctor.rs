use anyhow::Result;
use clap::Parser;
use locus_core::store::Store;

/// Diagnose and repair Locus installation
#[derive(Parser, Debug)]
pub struct DoctorCmd {
    /// Attempt automatic repairs
    #[arg(short, long)]
    pub fix: bool,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

impl DoctorCmd {
    pub fn run(self) -> Result<()> {
        let mut issues = Vec::new();
        let mut repairs = Vec::new();

        match Store::open_default() {
            Ok(store) => {
                // The search index can drift out of sync with canonical rows
                // (e.g. a previous interrupted reindex). That is the one
                // boot-time inconsistency Locus can safely auto-repair.
                match store.fts_out_of_sync() {
                    Ok(true) if self.fix => match store.reindex() {
                        Ok(count) => {
                            repairs.push(format!("Rebuilt search index ({count} rows)"));
                        }
                        Err(err) => issues.push(format!("Search index repair failed: {err}")),
                    },
                    Ok(true) => {
                        issues.push("Search index is out of sync with canonical rows".to_string());
                    }
                    Ok(false) => {}
                    Err(err) => issues.push(format!("Search index check failed: {err}")),
                }
            }
            Err(e) => {
                issues.push(format!("Database error: {e}"));
            }
        }

        if self.json {
            println!(
                "{}",
                serde_json::json!({
                    "status": if issues.is_empty() { "ok" } else { "issues-found" },
                    "issues": issues,
                    "repairs": repairs,
                })
            );
        } else {
            if issues.is_empty() {
                println!("✓ All checks passed");
            } else {
                println!("✗ Found {} issue(s):", issues.len());
                for issue in &issues {
                    println!("  - {issue}");
                }
            }

            if !repairs.is_empty() {
                println!("\nRepairs performed:");
                for repair in &repairs {
                    println!("  - {repair}");
                }
            }
        }

        Ok(())
    }
}
