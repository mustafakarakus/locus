//! `locus init` — install agent rules + MCP config into a project (U-008).

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use locus_core::init::{apply_plan, plan_init, ChangeAction};

/// Install Locus memory protocol into project rule files and MCP config
#[derive(Parser, Debug)]
pub struct InitCmd {
    /// Project root (default: current directory)
    #[arg(long, value_name = "DIR")]
    pub path: Option<PathBuf>,

    /// Apply changes without interactive confirmation
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,

    /// Show the plan / diff and exit without writing
    #[arg(long)]
    pub dry_run: bool,

    /// Output machine-readable JSON summary
    #[arg(long)]
    pub json: bool,
}

impl InitCmd {
    pub fn run(self) -> Result<()> {
        let root = self
            .path
            .clone()
            .unwrap_or_else(|| std::env::current_dir().expect("current dir"));

        let plan = plan_init(&root)
            .with_context(|| format!("failed to plan init for project root {}", root.display()))?;

        if self.json && (self.dry_run || plan.is_noop()) {
            print_json_plan(&plan, None)?;
            return Ok(());
        }

        if !self.json {
            print!("{}", plan.format_diff());
        }

        if plan.is_noop() {
            if self.json {
                print_json_plan(&plan, None)?;
            } else {
                println!("✓ Locus already initialized — nothing to do.");
            }
            return Ok(());
        }

        if self.dry_run {
            if self.json {
                print_json_plan(&plan, None)?;
            } else {
                println!("Dry run — no files written.");
            }
            return Ok(());
        }

        if !self.yes {
            if !stdin_is_tty() {
                bail!(
                    "refusing to modify files without confirmation \
                     (no TTY; re-run with --yes to apply non-interactively)"
                );
            }
            eprint!("Apply these changes? [y/N] ");
            let _ = io::stderr().flush();
            let mut answer = String::new();
            io::stdin()
                .read_line(&mut answer)
                .context("failed to read confirmation")?;
            let answer = answer.trim().to_ascii_lowercase();
            if answer != "y" && answer != "yes" {
                println!("Aborted — no files modified.");
                return Ok(());
            }
        }

        let result = apply_plan(&plan).context("failed to apply init plan")?;

        if self.json {
            print_json_plan(&plan, Some(&result))?;
        } else {
            println!();
            if result.written.is_empty() {
                println!("✓ Nothing written.");
            } else {
                println!("✓ Initialized Locus in {}", plan.project_root.display());
                for path in &result.written {
                    let rel = path
                        .strip_prefix(&plan.project_root)
                        .unwrap_or(path.as_path());
                    println!("  wrote  {}", rel.display());
                }
                for path in &result.backups {
                    let rel = path
                        .strip_prefix(&plan.project_root)
                        .unwrap_or(path.as_path());
                    println!("  backup {}", rel.display());
                }
                println!();
                println!("Agents will see the Locus Memory Protocol in rule files.");
                println!("MCP clients can use: locus mcp");
            }
        }

        Ok(())
    }
}

fn stdin_is_tty() -> bool {
    // Avoid pulling extra deps: treat non-terminal stdin as non-interactive.
    // `std::io::IsTerminal` is stable on our MSRV.
    use std::io::IsTerminal;
    io::stdin().is_terminal()
}

fn print_json_plan(
    plan: &locus_core::init::InitPlan,
    result: Option<&locus_core::init::InitResult>,
) -> Result<()> {
    use serde_json::json;

    let changes: Vec<_> = plan
        .rule_changes
        .iter()
        .chain(plan.mcp_changes.iter())
        .chain(plan.doc_changes.iter())
        .map(|c| {
            json!({
                "path": c.path,
                "label": c.label,
                "action": match c.action {
                    ChangeAction::Create => "create",
                    ChangeAction::Modify => "modify",
                    ChangeAction::Skip => "skip",
                },
                "summary": c.summary,
            })
        })
        .collect();

    let mut body = json!({
        "status": "ok",
        "project_name": plan.project_name,
        "project_type": plan.project_type.as_str(),
        "project_root": plan.project_root,
        "noop": plan.is_noop(),
        "changes": changes,
    });

    if let Some(r) = result {
        body["written"] = json!(r.written);
        body["skipped"] = json!(r.skipped);
        body["backups"] = json!(r.backups);
        body["applied"] = json!(true);
    } else {
        body["applied"] = json!(false);
    }

    println!("{body}");
    Ok(())
}
