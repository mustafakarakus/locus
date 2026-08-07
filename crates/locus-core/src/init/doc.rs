//! Detection and patching of project documentation files (U-015).
//!
//! `locus init` appends the doc-file `Locus Memory Protocol` block to
//! `README.md`, `CONTRIBUTING.md`, and `AGENTS.md` when those files exist, so
//! agents without a lifecycle hook system still get a passive fallback prompt.
//! Unlike rule files, doc files are never created — only patched when present.

use std::fs;
use std::path::Path;

use super::protocol::doc_protocol_is_installed;
use super::{ChangeAction, PlannedChange};
use crate::Result;

/// Project documentation files that receive the doc protocol block (U-015).
pub const DOC_FILE_NAMES: &[&str] = &["README.md", "CONTRIBUTING.md", "AGENTS.md"];

/// Return the names of doc files already present under `root`.
pub fn detect_doc_files(root: &Path) -> Vec<&'static str> {
    DOC_FILE_NAMES
        .iter()
        .copied()
        .filter(|name| root.join(name).is_file())
        .collect()
}

/// Plan a modify / skip for one doc file. Never plans a create.
pub fn plan_doc_change(root: &Path, file_name: &str, block: &str) -> Result<PlannedChange> {
    let path = root.join(file_name);

    let existing = if path.is_file() {
        fs::read_to_string(&path)?
    } else {
        String::new()
    };

    if existing.trim().is_empty() {
        return Ok(PlannedChange {
            path,
            action: ChangeAction::Skip,
            label: file_name.to_string(),
            proposed_content: String::new(),
            existing_content: existing,
            summary: "File absent — doc protocol block not added".into(),
        });
    }

    if doc_protocol_is_installed(&existing) {
        return Ok(PlannedChange {
            path,
            action: ChangeAction::Skip,
            label: file_name.to_string(),
            proposed_content: String::new(),
            existing_content: existing,
            summary: "Locus Memory Protocol already present — skip".into(),
        });
    }

    let mut proposed = existing.clone();
    if !proposed.ends_with('\n') {
        proposed.push('\n');
    }
    proposed.push('\n');
    proposed.push_str(block);
    if !proposed.ends_with('\n') {
        proposed.push('\n');
    }

    Ok(PlannedChange {
        path,
        action: ChangeAction::Modify,
        label: file_name.to_string(),
        proposed_content: proposed,
        existing_content: existing,
        summary: "Append Locus Memory Protocol block".into(),
    })
}

/// Write a planned doc-file change to disk (caller handles backups).
pub fn write_doc_change(change: &PlannedChange) -> Result<()> {
    if change.action == ChangeAction::Skip {
        return Ok(());
    }
    if let Some(parent) = change.path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Atomic-ish write via temp sibling (mirrors rules::write_rule_change).
    let tmp = change.path.with_file_name(format!(
        ".{}.locus-tmp",
        change
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
    ));
    fs::write(&tmp, &change.proposed_content)?;
    fs::rename(&tmp, &change.path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn detect_only_finds_present_doc_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("README.md"), "readme\n").unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "agents\n").unwrap();
        let found = detect_doc_files(tmp.path());
        assert_eq!(found, vec!["README.md", "AGENTS.md"]);
    }

    #[test]
    fn absent_doc_file_plans_skip() {
        let tmp = TempDir::new().unwrap();
        let change = plan_doc_change(tmp.path(), "README.md", "# block").unwrap();
        assert_eq!(change.action, ChangeAction::Skip);
        assert!(change.summary.contains("absent"));
    }
}
