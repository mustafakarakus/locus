//! Project initialization: agent rules + MCP config + doc protocol (`locus init`).
//!
//! Installs a visible **Locus Memory Protocol** into the rule files AI coding
//! agents already read (`.cursorrules`, `CLAUDE.md`, `.clinerules`), merges a
//! `locus mcp` server entry into project MCP configs, and (U-015) appends the
//! doc-file protocol block to `README.md` / `CONTRIBUTING.md` / `AGENTS.md`
//! when present — the passive fallback tier for agents without a hook system.
//!
//! Design goals (U-008, U-015):
//! - idempotent (markers prevent duplicate blocks)
//! - never silently overwrite user content (append / merge only)
//! - show a plan / diff before writing; caller confirms
//! - backup existing files before first modification

mod doc;
mod mcp;
mod protocol;
mod rules;

pub use doc::{detect_doc_files, plan_doc_change, write_doc_change, DOC_FILE_NAMES};
pub use mcp::{mcp_server_entry, McpConfigTarget};
pub use protocol::{
    doc_protocol_block, doc_protocol_is_installed, protocol_block, protocol_is_installed,
    DOC_PROTOCOL_END_MARKER, DOC_PROTOCOL_START_MARKER, PROTOCOL_END_MARKER, PROTOCOL_START_MARKER,
};
pub use rules::{detect_rule_files, RuleFileKind, RULE_FILE_NAMES};

use std::fs;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

use mcp::{detect_mcp_configs, plan_claude_hooks_change, plan_mcp_change, write_mcp_change};
use rules::{plan_rule_change, write_rule_change};

/// High-level project classification used for messaging and namespace hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectType {
    Rust,
    Node,
    Python,
    Go,
    Java,
    Ruby,
    Mixed,
    Unknown,
}

impl ProjectType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Node => "node",
            Self::Python => "python",
            Self::Go => "go",
            Self::Java => "java",
            Self::Ruby => "ruby",
            Self::Mixed => "mixed",
            Self::Unknown => "unknown",
        }
    }
}

/// What will happen to a single file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeAction {
    /// File does not exist; will be created.
    Create,
    /// File exists; protocol / entry will be appended or merged.
    Modify,
    /// Already has the Locus content; no write needed.
    Skip,
}

/// A planned change to one project file.
#[derive(Debug, Clone)]
pub struct PlannedChange {
    pub path: PathBuf,
    pub action: ChangeAction,
    /// Human-readable label (e.g. "CLAUDE.md", ".cursor/mcp.json").
    pub label: String,
    /// Full content that would be written (for Create/Modify). Empty for Skip.
    pub proposed_content: String,
    /// Current file content when modifying, else empty.
    pub existing_content: String,
    /// Short description of the change for CLI output.
    pub summary: String,
}

/// Full init plan for a project root. Inspect, print, then [`apply_plan`].
#[derive(Debug, Clone)]
pub struct InitPlan {
    pub project_root: PathBuf,
    pub project_name: String,
    pub project_type: ProjectType,
    pub rule_changes: Vec<PlannedChange>,
    pub mcp_changes: Vec<PlannedChange>,
    pub doc_changes: Vec<PlannedChange>,
}

impl InitPlan {
    /// True when every planned change is already applied (idempotent re-run).
    pub fn is_noop(&self) -> bool {
        self.rule_changes
            .iter()
            .chain(self.mcp_changes.iter())
            .chain(self.doc_changes.iter())
            .all(|c| c.action == ChangeAction::Skip)
    }

    /// Changes that would write to disk.
    pub fn pending(&self) -> impl Iterator<Item = &PlannedChange> {
        self.rule_changes
            .iter()
            .chain(self.mcp_changes.iter())
            .chain(self.doc_changes.iter())
            .filter(|c| c.action != ChangeAction::Skip)
    }

    /// Render a human-readable plan / diff summary.
    pub fn format_diff(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Project: {} ({})\nRoot: {}\n\n",
            self.project_name,
            self.project_type.as_str(),
            self.project_root.display()
        ));

        if self.is_noop() {
            out.push_str("Nothing to do — Locus is already initialized.\n");
            return out;
        }

        out.push_str("Planned changes:\n");
        for change in self.pending() {
            let verb = match change.action {
                ChangeAction::Create => "create",
                ChangeAction::Modify => "modify",
                ChangeAction::Skip => continue,
            };
            out.push_str(&format!(
                "\n  [{verb}] {}\n    {}\n",
                change.path.display(),
                change.summary
            ));
            if change.action == ChangeAction::Modify && !change.existing_content.is_empty() {
                // Show only the added tail for readability.
                let added = diff_added_tail(&change.existing_content, &change.proposed_content);
                for line in added.lines().take(40) {
                    out.push_str(&format!("    + {line}\n"));
                }
                let total = added.lines().count();
                if total > 40 {
                    out.push_str(&format!("    + … ({} more lines)\n", total - 40));
                }
            } else if change.action == ChangeAction::Create {
                for line in change.proposed_content.lines().take(20) {
                    out.push_str(&format!("    + {line}\n"));
                }
                let total = change.proposed_content.lines().count();
                if total > 20 {
                    out.push_str(&format!("    + … ({} more lines)\n", total - 20));
                }
            }
        }

        out.push_str(
            "\nExisting files will be backed up as `<name>.locus-backup` before writing.\n",
        );
        out
    }
}

/// Result of applying an [`InitPlan`].
#[derive(Debug, Clone, Default)]
pub struct InitResult {
    pub written: Vec<PathBuf>,
    pub skipped: Vec<PathBuf>,
    pub backups: Vec<PathBuf>,
}

/// Detect project type from well-known manifest files in `root`.
pub fn detect_project_type(root: &Path) -> ProjectType {
    let mut hits = Vec::new();
    if root.join("Cargo.toml").is_file() {
        hits.push(ProjectType::Rust);
    }
    if root.join("package.json").is_file() {
        hits.push(ProjectType::Node);
    }
    if root.join("pyproject.toml").is_file()
        || root.join("setup.py").is_file()
        || root.join("requirements.txt").is_file()
    {
        hits.push(ProjectType::Python);
    }
    if root.join("go.mod").is_file() {
        hits.push(ProjectType::Go);
    }
    if root.join("pom.xml").is_file()
        || root.join("build.gradle").is_file()
        || root.join("build.gradle.kts").is_file()
    {
        hits.push(ProjectType::Java);
    }
    if root.join("Gemfile").is_file() {
        hits.push(ProjectType::Ruby);
    }

    match hits.len() {
        0 => ProjectType::Unknown,
        1 => hits[0].clone(),
        _ => ProjectType::Mixed,
    }
}

/// Derive a short project name from the directory or, for Rust, `Cargo.toml`.
pub fn detect_project_name(root: &Path) -> String {
    // Prefer Cargo package name when present.
    let cargo = root.join("Cargo.toml");
    if cargo.is_file() {
        if let Ok(text) = fs::read_to_string(&cargo) {
            if let Some(name) = parse_cargo_package_name(&text) {
                return name;
            }
        }
    }
    // package.json "name"
    let pkg = root.join("package.json");
    if pkg.is_file() {
        if let Ok(text) = fs::read_to_string(&pkg) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                    let cleaned = name.trim().trim_start_matches('@');
                    // scoped packages: @scope/name → name
                    if let Some((_, rest)) = cleaned.split_once('/') {
                        if !rest.is_empty() {
                            return sanitize_name(rest);
                        }
                    }
                    if !cleaned.is_empty() {
                        return sanitize_name(cleaned);
                    }
                }
            }
        }
    }

    root.file_name()
        .and_then(|s| s.to_str())
        .map(sanitize_name)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "project".to_string())
}

/// Build an init plan for `project_root` without touching the filesystem
/// (aside from reads).
pub fn plan_init(project_root: &Path) -> Result<InitPlan> {
    let root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());

    if !root.is_dir() {
        return Err(Error::InvalidInput(format!(
            "project root is not a directory: {}",
            root.display()
        )));
    }

    let project_name = detect_project_name(&root);
    let project_type = detect_project_type(&root);
    let block = protocol_block(&project_name);

    // Rule files: patch any that exist; if none exist, create the standard set.
    let existing_rules = detect_rule_files(&root);
    let rule_targets: Vec<RuleFileKind> = if existing_rules.is_empty() {
        vec![
            RuleFileKind::CursorRules,
            RuleFileKind::ClaudeMd,
            RuleFileKind::ClineRules,
        ]
    } else {
        existing_rules
    };

    let rule_changes = rule_targets
        .into_iter()
        .map(|kind| plan_rule_change(&root, kind, &block))
        .collect::<Result<Vec<_>>>()?;

    // MCP configs: patch any that exist; if none, create the standard
    // project-level configs for Claude Code, Cursor, and VS Code/Copilot.
    let existing_mcp = detect_mcp_configs(&root);
    let mcp_targets: Vec<McpConfigTarget> = if existing_mcp.is_empty() {
        vec![
            McpConfigTarget::McpJson,
            McpConfigTarget::CursorMcp,
            McpConfigTarget::VsCodeMcp,
        ]
    } else {
        existing_mcp
    };

    let mut mcp_changes = mcp_targets
        .into_iter()
        .map(|target| plan_mcp_change(&root, target))
        .collect::<Result<Vec<_>>>()?;
    mcp_changes.push(plan_claude_hooks_change(&root)?);

    // Doc files (U-015): passive fallback for agents without a hook system.
    // Only patched when present — never created.
    let doc_block = doc_protocol_block(&project_name);
    let doc_changes = detect_doc_files(&root)
        .into_iter()
        .map(|name| plan_doc_change(&root, name, &doc_block))
        .collect::<Result<Vec<_>>>()?;

    Ok(InitPlan {
        project_root: root,
        project_name,
        project_type,
        rule_changes,
        mcp_changes,
        doc_changes,
    })
}

/// Apply a previously computed plan. Creates backups for modified files.
pub fn apply_plan(plan: &InitPlan) -> Result<InitResult> {
    let mut result = InitResult::default();

    for change in &plan.rule_changes {
        apply_one(change, WriteKind::Rule, &mut result)?;
    }
    for change in &plan.mcp_changes {
        apply_one(change, WriteKind::Mcp, &mut result)?;
    }
    for change in &plan.doc_changes {
        apply_one(change, WriteKind::Doc, &mut result)?;
    }

    Ok(result)
}

enum WriteKind {
    Rule,
    Mcp,
    Doc,
}

fn apply_one(change: &PlannedChange, kind: WriteKind, result: &mut InitResult) -> Result<()> {
    match change.action {
        ChangeAction::Skip => {
            result.skipped.push(change.path.clone());
            Ok(())
        }
        ChangeAction::Create | ChangeAction::Modify => {
            if change.action == ChangeAction::Modify && change.path.is_file() {
                let backup = backup_file(&change.path)?;
                result.backups.push(backup);
            }
            match kind {
                WriteKind::Rule => write_rule_change(change)?,
                WriteKind::Mcp => write_mcp_change(change)?,
                WriteKind::Doc => write_doc_change(change)?,
            }
            result.written.push(change.path.clone());
            Ok(())
        }
    }
}

/// Convenience: plan + apply in one call (no interactive confirmation).
pub fn init_project(project_root: &Path) -> Result<(InitPlan, InitResult)> {
    let plan = plan_init(project_root)?;
    let result = apply_plan(&plan)?;
    Ok((plan, result))
}

/// Copy `path` to `path` + `.locus-backup` (or `.locus-backup.N` if taken).
pub fn backup_file(path: &Path) -> Result<PathBuf> {
    let backup = unique_backup_path(path);
    fs::copy(path, &backup)?;
    Ok(backup)
}

fn unique_backup_path(path: &Path) -> PathBuf {
    let base = path.with_file_name(format!(
        "{}.locus-backup",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("file")
    ));
    if !base.exists() {
        return base;
    }
    for n in 1..1000 {
        let candidate = path.with_file_name(format!(
            "{}.locus-backup.{}",
            path.file_name().and_then(|s| s.to_str()).unwrap_or("file"),
            n
        ));
        if !candidate.exists() {
            return candidate;
        }
    }
    base
}

fn diff_added_tail(existing: &str, proposed: &str) -> String {
    if let Some(stripped) = proposed.strip_prefix(existing) {
        stripped.trim_start_matches('\n').to_string()
    } else if let Some(idx) = proposed.find(PROTOCOL_START_MARKER) {
        proposed[idx..].to_string()
    } else if let Some(idx) = proposed.find(DOC_PROTOCOL_START_MARKER) {
        proposed[idx..].to_string()
    } else {
        // JSON merge: show whole proposed for simplicity.
        proposed.to_string()
    }
}

fn parse_cargo_package_name(toml: &str) -> Option<String> {
    // Minimal parse: look for [package] then name = "..."
    let mut in_package = false;
    for line in toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = trimmed.strip_prefix("name") {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('=') {
                    let rest = rest.trim();
                    let name = rest.trim_matches('"').trim_matches('\'').trim();
                    if !name.is_empty() {
                        return Some(sanitize_name(name));
                    }
                }
            }
        }
    }
    None
}

fn sanitize_name(raw: &str) -> String {
    let s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "project".into()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn fresh_project_gets_rules_block() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let plan = plan_init(root).unwrap();
        assert!(!plan.is_noop());
        let result = apply_plan(&plan).unwrap();
        assert!(!result.written.is_empty());

        for name in RULE_FILE_NAMES {
            let path = root.join(name);
            assert!(path.is_file(), "expected {name} to be created");
            let text = fs::read_to_string(&path).unwrap();
            assert!(protocol_is_installed(&text), "{name} missing protocol");
            assert!(text.contains("memory_search"));
            assert!(text.contains("memory_save"));
            assert!(
                text.contains("Do not save secrets")
                    || text.contains("do not save secrets")
                    || text.contains("secrets")
            );
            assert!(text.contains("NO_RELEVANT_MEMORY"));
        }
    }

    #[test]
    fn existing_file_is_appended_safely() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let original = "# My Project Rules\n\nAlways use rustfmt.\n";
        write(root, "CLAUDE.md", original);

        let plan = plan_init(root).unwrap();
        // Only CLAUDE.md among rules (existing detected); may still create MCP.
        let claude = plan
            .rule_changes
            .iter()
            .find(|c| c.label == "CLAUDE.md")
            .expect("CLAUDE.md change");
        assert_eq!(claude.action, ChangeAction::Modify);

        apply_plan(&plan).unwrap();

        let text = fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        assert!(text.starts_with("# My Project Rules"));
        assert!(text.contains("Always use rustfmt."));
        assert!(protocol_is_installed(&text));
        // User content preserved exactly at the start.
        assert!(text.contains("Always use rustfmt."));
    }

    #[test]
    fn repeated_init_does_not_duplicate_block() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let (plan1, _) = init_project(root).unwrap();
        assert!(!plan1.is_noop());

        let text1 = fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        let count1 = text1.matches(PROTOCOL_START_MARKER).count();
        assert_eq!(count1, 1);

        let plan2 = plan_init(root).unwrap();
        assert!(plan2.is_noop(), "second init should be a no-op");
        apply_plan(&plan2).unwrap();

        let text2 = fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        assert_eq!(text1, text2);
        assert_eq!(text2.matches(PROTOCOL_START_MARKER).count(), 1);
    }

    #[test]
    fn backup_is_created_on_modify() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, ".cursorrules", "user rules here\n");

        let plan = plan_init(root).unwrap();
        let result = apply_plan(&plan).unwrap();

        assert!(
            !result.backups.is_empty(),
            "expected at least one backup, got none"
        );
        let backup = &result.backups[0];
        assert!(backup.is_file());
        let backup_text = fs::read_to_string(backup).unwrap();
        assert_eq!(backup_text, "user rules here\n");

        // Original path now has protocol appended.
        let updated = fs::read_to_string(root.join(".cursorrules")).unwrap();
        assert!(updated.contains("user rules here"));
        assert!(protocol_is_installed(&updated));
    }

    #[test]
    fn mcp_config_remains_valid_json() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(
            root,
            ".mcp.json",
            r#"{
  "mcpServers": {
    "other": {
      "command": "other-server"
    }
  }
}
"#,
        );

        let plan = plan_init(root).unwrap();
        apply_plan(&plan).unwrap();

        let text = fs::read_to_string(root.join(".mcp.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        let servers = value.get("mcpServers").expect("mcpServers key");
        assert!(servers.get("other").is_some(), "existing server preserved");
        assert!(servers.get("locus").is_some(), "locus server added");
        let locus = &servers["locus"];
        assert_eq!(locus["command"], "locus");
        assert_eq!(locus["args"], serde_json::json!(["mcp"]));
    }

    #[test]
    fn fresh_init_creates_mcp_configs_for_all_supported_hosts() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let plan = plan_init(root).unwrap();
        apply_plan(&plan).unwrap();

        for relative in [".mcp.json", ".cursor/mcp.json", ".vscode/mcp.json"] {
            let text = fs::read_to_string(root.join(relative)).expect("MCP config created");
            let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
            let key = if relative == ".vscode/mcp.json" {
                "servers"
            } else {
                "mcpServers"
            };
            assert!(
                value[key]["locus"].is_object(),
                "{relative} must configure Locus"
            );
        }

        let claude: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(root.join(".claude/settings.json"))
                .expect("Claude settings created"),
        )
        .expect("valid Claude settings");
        assert!(claude["hooks"]["SessionStart"].is_array());
        assert!(claude["hooks"]["UserPromptSubmit"].is_array());
        assert!(claude["hooks"]["PostCompact"].is_array());
    }

    #[test]
    fn init_does_not_corrupt_existing_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let original = "line1\nline2\nline3\n# Important heading\nkeep me\n";
        write(root, ".clinerules", original);

        init_project(root).unwrap();

        let text = fs::read_to_string(root.join(".clinerules")).unwrap();
        // Every original line still present in order at the start.
        assert!(
            text.starts_with(original.trim_end()) || text.starts_with(original),
            "user content corrupted:\n{text}"
        );
        assert!(text.contains("keep me"));
        assert!(protocol_is_installed(&text));
    }

    #[test]
    fn detect_rust_project_type_and_name() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(
            root,
            "Cargo.toml",
            "[package]\nname = \"my-awesome-app\"\nversion = \"0.1.0\"\n",
        );
        assert_eq!(detect_project_type(root), ProjectType::Rust);
        assert_eq!(detect_project_name(root), "my-awesome-app");
    }

    #[test]
    fn detect_existing_rule_and_mcp_files_only() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "CLAUDE.md", "hello\n");
        write(root, ".cursor/mcp.json", "{\"mcpServers\":{}}\n");

        let plan = plan_init(root).unwrap();
        let rule_labels: Vec<_> = plan.rule_changes.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(rule_labels, vec!["CLAUDE.md"]);
        let mcp_labels: Vec<_> = plan.mcp_changes.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(mcp_labels, vec![".cursor/mcp.json"]);
    }

    #[test]
    fn format_diff_mentions_files() {
        let tmp = TempDir::new().unwrap();
        let plan = plan_init(tmp.path()).unwrap();
        let diff = plan.format_diff();
        assert!(diff.contains("Planned changes") || diff.contains("create"));
        assert!(diff.contains("CLAUDE.md") || diff.contains(".cursorrules"));
    }

    #[test]
    fn protocol_contains_required_instructions() {
        let block = protocol_block("demo");
        assert!(block.contains(PROTOCOL_START_MARKER));
        assert!(block.contains(PROTOCOL_END_MARKER));
        assert!(block.contains("memory_search"));
        assert!(block.contains("memory_save"));
        assert!(block.contains("secrets"));
        assert!(block.contains("NO_RELEVANT_MEMORY"));
        assert!(block.contains("project:demo"));
    }

    #[test]
    fn doc_block_is_written_to_present_doc_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(
            root,
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        );
        write(root, "README.md", "# Demo\n\nIntro.\n");
        write(root, "CONTRIBUTING.md", "# Contributing\n\nThanks!\n");
        write(root, "AGENTS.md", "# Agents\n\nFollow the rules.\n");

        let plan = plan_init(root).unwrap();
        let doc_labels: Vec<_> = plan.doc_changes.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            doc_labels,
            vec!["README.md", "CONTRIBUTING.md", "AGENTS.md"]
        );
        assert!(
            plan.doc_changes
                .iter()
                .all(|c| c.action == ChangeAction::Modify),
            "all present doc files should be modified"
        );

        apply_plan(&plan).unwrap();

        for name in ["README.md", "CONTRIBUTING.md", "AGENTS.md"] {
            let text = fs::read_to_string(root.join(name)).unwrap();
            assert!(
                doc_protocol_is_installed(&text),
                "{name} missing doc protocol"
            );
            assert!(text.contains("locus context"), "{name} missing CLI form");
            assert!(text.contains("memory_search"), "{name} missing MCP form");
            assert!(
                text.contains("project:demo"),
                "{name} missing namespace hint"
            );
        }
    }

    #[test]
    fn doc_files_are_never_created_when_absent() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let plan = plan_init(root).unwrap();
        assert!(plan.doc_changes.is_empty());

        apply_plan(&plan).unwrap();
        for name in DOC_FILE_NAMES {
            assert!(!root.join(name).exists(), "should not create {name}");
        }
    }

    #[test]
    fn repeated_init_does_not_duplicate_doc_block() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "README.md", "# Demo\n");

        let (plan1, _) = init_project(root).unwrap();
        assert!(plan1
            .doc_changes
            .iter()
            .any(|c| c.action == ChangeAction::Modify));

        let text1 = fs::read_to_string(root.join("README.md")).unwrap();
        let count1 = text1.matches(DOC_PROTOCOL_START_MARKER).count();
        assert_eq!(count1, 1);

        let plan2 = plan_init(root).unwrap();
        assert!(plan2.is_noop(), "second init should be a no-op");
        apply_plan(&plan2).unwrap();

        let text2 = fs::read_to_string(root.join("README.md")).unwrap();
        assert_eq!(text1, text2);
        assert_eq!(text2.matches(DOC_PROTOCOL_START_MARKER).count(), 1);
    }

    #[test]
    fn doc_block_keeps_user_content_untouched() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "README.md", "keep me exactly\n");

        let (plan, _) = init_project(root).unwrap();
        let readme = plan
            .doc_changes
            .iter()
            .find(|c| c.label == "README.md")
            .expect("README change");
        assert_eq!(readme.action, ChangeAction::Modify);
        assert!(readme.proposed_content.starts_with("keep me exactly"));

        apply_plan(&plan).unwrap();
        let text = fs::read_to_string(root.join("README.md")).unwrap();
        assert!(text.starts_with("keep me exactly"));
        assert!(doc_protocol_is_installed(&text));
    }
}
