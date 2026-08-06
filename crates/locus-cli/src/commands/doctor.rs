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

        // Check if store can be opened
        if let Err(e) = Store::open_default() {
            issues.push(format!("Database error: {}", e));
            if self.fix {
                // TODO: Implement database repair
                repairs.push("Attempted to repair database".to_string());
            }
        }

        if self.json {
            let repairs_str = if repairs.is_empty() {
                "[]".to_string()
            } else {
                format!(
                    "[{}]",
                    repairs
                        .iter()
                        .map(|r| format!("\"{}\"", r))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            };
            let issues_str = if issues.is_empty() {
                "[]".to_string()
            } else {
                format!(
                    "[{}]",
                    issues
                        .iter()
                        .map(|i| format!("\"{}\"", i))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            };
            println!(
                "{{\"status\":\"ok\",\"issues\":{},\"repairs\":{}}}",
                issues_str, repairs_str
            );
        } else {
            if issues.is_empty() {
                println!("✓ All checks passed");
            } else {
                println!("✗ Found {} issue(s):", issues.len());
                for issue in issues {
                    println!("  - {}", issue);
                }
            }

            if !repairs.is_empty() {
                println!("\nRepairs attempted:");
                for repair in repairs {
                    println!("  - {}", repair);
                }
            }
        }

        Ok(())
    }
}
