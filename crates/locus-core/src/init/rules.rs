//! Detection and patching of agent rule files.

use std::fs;
use std::path::{Path, PathBuf};

use super::protocol::{append_protocol, protocol_is_installed};
use super::{ChangeAction, PlannedChange};
use crate::Result;

/// Well-known agent rule file basenames (U-008).
pub const RULE_FILE_NAMES: &[&str] = &[".cursorrules", "CLAUDE.md", ".clinerules"];

/// Which rule file we are targeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleFileKind {
    CursorRules,
    ClaudeMd,
    ClineRules,
}

impl RuleFileKind {
    pub fn file_name(self) -> &'static str {
        match self {
            Self::CursorRules => ".cursorrules",
            Self::ClaudeMd => "CLAUDE.md",
            Self::ClineRules => ".clinerules",
        }
    }

    pub fn label(self) -> &'static str {
        self.file_name()
    }

    pub fn path(self, root: &Path) -> PathBuf {
        root.join(self.file_name())
    }

    pub fn from_file_name(name: &str) -> Option<Self> {
        match name {
            ".cursorrules" => Some(Self::CursorRules),
            "CLAUDE.md" => Some(Self::ClaudeMd),
            ".clinerules" => Some(Self::ClineRules),
            _ => None,
        }
    }
}

/// Return rule file kinds that already exist under `root`.
pub fn detect_rule_files(root: &Path) -> Vec<RuleFileKind> {
    [
        RuleFileKind::CursorRules,
        RuleFileKind::ClaudeMd,
        RuleFileKind::ClineRules,
    ]
    .into_iter()
    .filter(|k| k.path(root).is_file())
    .collect()
}

/// Plan a create / modify / skip for one rule file.
pub fn plan_rule_change(
    root: &Path,
    kind: RuleFileKind,
    protocol_block: &str,
) -> Result<PlannedChange> {
    let path = kind.path(root);
    if path.is_file() {
        let existing = fs::read_to_string(&path)?;
        if protocol_is_installed(&existing) {
            return Ok(PlannedChange {
                path,
                action: ChangeAction::Skip,
                label: kind.label().to_string(),
                proposed_content: String::new(),
                existing_content: existing,
                summary: "Locus Memory Protocol already present — skip".into(),
            });
        }
        // Re-derive project name from block is awkward; append via helper that
        // rebuilds from markers already in protocol_block by concatenating.
        let mut proposed = existing.clone();
        if !proposed.is_empty() && !proposed.ends_with('\n') {
            proposed.push('\n');
        }
        if !proposed.is_empty() {
            proposed.push('\n');
        }
        proposed.push_str(protocol_block);
        if !proposed.ends_with('\n') {
            proposed.push('\n');
        }
        Ok(PlannedChange {
            path,
            action: ChangeAction::Modify,
            label: kind.label().to_string(),
            proposed_content: proposed,
            existing_content: existing,
            summary: "Append Locus Memory Protocol block".into(),
        })
    } else {
        let mut proposed = protocol_block.to_string();
        if !proposed.ends_with('\n') {
            proposed.push('\n');
        }
        Ok(PlannedChange {
            path,
            action: ChangeAction::Create,
            label: kind.label().to_string(),
            proposed_content: proposed,
            existing_content: String::new(),
            summary: "Create with Locus Memory Protocol block".into(),
        })
    }
}

/// Write a planned rule-file change to disk (caller handles backups).
pub fn write_rule_change(change: &PlannedChange) -> Result<()> {
    if change.action == ChangeAction::Skip {
        return Ok(());
    }
    if let Some(parent) = change.path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Atomic-ish write via temp sibling.
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

/// Test helper path — re-export append for callers that have a project name.
#[allow(dead_code)]
pub fn append_protocol_to_file(path: &Path, project_name: &str) -> Result<bool> {
    let existing = if path.is_file() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    let (new_content, changed) = append_protocol(&existing, project_name);
    if changed {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, new_content)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn detect_finds_present_files_only() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "x").unwrap();
        let found = detect_rule_files(tmp.path());
        assert_eq!(found, vec![RuleFileKind::ClaudeMd]);
    }
}
