//! Detection and merging of project MCP server configs for `locus mcp`.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::{ChangeAction, PlannedChange};
use crate::{Error, Result};

/// Project-level MCP config locations we know how to manage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConfigTarget {
    /// Claude Code project config: `.mcp.json`
    McpJson,
    /// Cursor project config: `.cursor/mcp.json`
    CursorMcp,
    /// VS Code / Copilot-style: `.vscode/mcp.json`
    VsCodeMcp,
}

impl McpConfigTarget {
    pub fn relative_path(self) -> &'static str {
        match self {
            Self::McpJson => ".mcp.json",
            Self::CursorMcp => ".cursor/mcp.json",
            Self::VsCodeMcp => ".vscode/mcp.json",
        }
    }

    pub fn label(self) -> &'static str {
        self.relative_path()
    }

    pub fn path(self, root: &Path) -> PathBuf {
        root.join(self.relative_path())
    }

    /// JSON object key that holds the server map.
    ///
    /// Cursor / Claude Code use `mcpServers`. VS Code newer MCP configs often
    /// use `servers`.
    pub fn servers_key(self) -> &'static str {
        match self {
            Self::McpJson | Self::CursorMcp => "mcpServers",
            Self::VsCodeMcp => "servers",
        }
    }
}

/// JSON object for the `locus` MCP server entry.
pub fn mcp_server_entry(target: McpConfigTarget) -> Value {
    match target {
        McpConfigTarget::VsCodeMcp => json!({
            "command": "locus",
            "args": ["mcp"],
            "type": "stdio"
        }),
        _ => json!({
            "command": "locus",
            "args": ["mcp"]
        }),
    }
}

/// Return MCP config targets that already exist under `root`.
pub fn detect_mcp_configs(root: &Path) -> Vec<McpConfigTarget> {
    [
        McpConfigTarget::McpJson,
        McpConfigTarget::CursorMcp,
        McpConfigTarget::VsCodeMcp,
    ]
    .into_iter()
    .filter(|t| t.path(root).is_file())
    .collect()
}

/// Plan create / modify / skip for one MCP config file.
pub fn plan_mcp_change(root: &Path, target: McpConfigTarget) -> Result<PlannedChange> {
    let path = target.path(root);
    let key = target.servers_key();
    let entry = mcp_server_entry(target);

    if path.is_file() {
        let existing = fs::read_to_string(&path)?;
        let mut value: Value = serde_json::from_str(&existing).map_err(|e| {
            Error::InvalidInput(format!(
                "MCP config {} is not valid JSON: {e}",
                path.display()
            ))
        })?;

        if locus_already_configured(&value, key) {
            return Ok(PlannedChange {
                path,
                action: ChangeAction::Skip,
                label: target.label().to_string(),
                proposed_content: String::new(),
                existing_content: existing,
                summary: "locus MCP server already configured — skip".into(),
            });
        }

        merge_locus_server(&mut value, key, entry)?;
        let proposed = pretty_json(&value)?;
        Ok(PlannedChange {
            path,
            action: ChangeAction::Modify,
            label: target.label().to_string(),
            proposed_content: proposed,
            existing_content: existing,
            summary: format!("Merge locus MCP server into `{key}`"),
        })
    } else {
        let mut map = Map::new();
        let mut servers = Map::new();
        servers.insert("locus".into(), entry);
        map.insert(key.into(), Value::Object(servers));
        let proposed = pretty_json(&Value::Object(map))?;
        Ok(PlannedChange {
            path,
            action: ChangeAction::Create,
            label: target.label().to_string(),
            proposed_content: proposed,
            existing_content: String::new(),
            summary: format!("Create MCP config with locus server under `{key}`"),
        })
    }
}

/// Plan Claude Code lifecycle hooks for context injection and compaction
/// capture. Existing settings and hook entries are preserved.
pub fn plan_claude_hooks_change(root: &Path) -> Result<PlannedChange> {
    let path = root.join(".claude/settings.json");
    let existing = if path.is_file() {
        fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let mut value: Value = if existing.is_empty() {
        json!({})
    } else {
        serde_json::from_str(&existing).map_err(|e| {
            Error::InvalidInput(format!(
                "Claude settings {} are not valid JSON: {e}",
                path.display()
            ))
        })?
    };
    if !value.is_object() {
        return Err(Error::InvalidInput(
            "Claude settings root must be a JSON object".into(),
        ));
    }

    let desired = [
        (
            "SessionStart",
            json!({
                "matcher": "startup|resume|clear|compact|fork",
                "hooks": [{
                    "type": "command",
                    "command": "locus",
                    "args": ["hook", "context", "--host", "claude-code"],
                    "timeout": 5
                }]
            }),
        ),
        (
            "UserPromptSubmit",
            json!({
                "hooks": [{
                    "type": "command",
                    "command": "locus",
                    "args": ["hook", "context", "--host", "claude-code"],
                    "timeout": 5
                }]
            }),
        ),
        (
            "PostCompact",
            json!({
                "matcher": "manual|auto",
                "hooks": [{
                    "type": "command",
                    "command": "locus",
                    "args": ["hook", "capture", "--host", "claude-code"],
                    "async": true,
                    "timeout": 10
                }]
            }),
        ),
    ];

    let root_obj = value.as_object_mut().expect("checked object");
    let hooks = root_obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks_obj = hooks.as_object_mut().ok_or_else(|| {
        Error::InvalidInput("Claude settings `hooks` must be a JSON object".into())
    })?;
    let mut changed = false;
    for (event, entry) in desired {
        let entries = hooks_obj
            .entry(event.to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| {
                Error::InvalidInput(format!(
                    "Claude settings hook event `{event}` must be an array"
                ))
            })?;
        if !entries.contains(&entry) {
            entries.push(entry);
            changed = true;
        }
    }

    if !changed {
        return Ok(PlannedChange {
            path,
            action: ChangeAction::Skip,
            label: ".claude/settings.json".into(),
            proposed_content: String::new(),
            existing_content: existing,
            summary: "Claude lifecycle hooks already configured — skip".into(),
        });
    }

    Ok(PlannedChange {
        path,
        action: if existing.is_empty() {
            ChangeAction::Create
        } else {
            ChangeAction::Modify
        },
        label: ".claude/settings.json".into(),
        proposed_content: pretty_json(&value)?,
        existing_content: existing,
        summary: "Configure Claude context and compaction hooks".into(),
    })
}

/// Write planned MCP content (caller handles backups / parent dirs).
pub fn write_mcp_change(change: &PlannedChange) -> Result<()> {
    if change.action == ChangeAction::Skip {
        return Ok(());
    }
    if let Some(parent) = change.path.parent() {
        fs::create_dir_all(parent)?;
    }
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

fn locus_already_configured(value: &Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(|s| s.as_object())
        .is_some_and(|servers| servers.contains_key("locus"))
}

fn merge_locus_server(value: &mut Value, key: &str, entry: Value) -> Result<()> {
    if !value.is_object() {
        return Err(Error::InvalidInput(
            "MCP config root must be a JSON object".into(),
        ));
    }
    let obj = value.as_object_mut().expect("checked is_object");
    let servers = obj.entry(key.to_string()).or_insert_with(|| json!({}));
    if !servers.is_object() {
        return Err(Error::InvalidInput(format!(
            "MCP config `{key}` must be a JSON object"
        )));
    }
    servers
        .as_object_mut()
        .expect("checked is_object")
        .insert("locus".into(), entry);
    Ok(())
}

fn pretty_json(value: &Value) -> Result<String> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn merge_preserves_other_servers() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(".mcp.json");
        fs::write(
            &path,
            r#"{"mcpServers":{"alpha":{"command":"a"},"beta":{"command":"b"}}}"#,
        )
        .unwrap();

        let change = plan_mcp_change(tmp.path(), McpConfigTarget::McpJson).unwrap();
        assert_eq!(change.action, ChangeAction::Modify);
        write_mcp_change(&change).unwrap();

        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let servers = v["mcpServers"].as_object().unwrap();
        assert!(servers.contains_key("alpha"));
        assert!(servers.contains_key("beta"));
        assert!(servers.contains_key("locus"));
    }

    #[test]
    fn skip_when_locus_present() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join(".mcp.json"),
            r#"{"mcpServers":{"locus":{"command":"locus","args":["mcp"]}}}"#,
        )
        .unwrap();
        let change = plan_mcp_change(tmp.path(), McpConfigTarget::McpJson).unwrap();
        assert_eq!(change.action, ChangeAction::Skip);
    }

    #[test]
    fn create_cursor_mcp() {
        let tmp = TempDir::new().unwrap();
        let change = plan_mcp_change(tmp.path(), McpConfigTarget::CursorMcp).unwrap();
        assert_eq!(change.action, ChangeAction::Create);
        write_mcp_change(&change).unwrap();
        assert!(tmp.path().join(".cursor/mcp.json").is_file());
    }

    #[test]
    fn claude_hooks_preserve_existing_settings_and_hooks() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(".claude/settings.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"permissions":{"allow":["Read"]},"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"existing"}]}]}}"#,
        )
        .unwrap();

        let change = plan_claude_hooks_change(tmp.path()).unwrap();
        assert_eq!(change.action, ChangeAction::Modify);
        write_mcp_change(&change).unwrap();

        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["permissions"]["allow"], json!(["Read"]));
        assert_eq!(value["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
        assert!(value["hooks"]["PostCompact"][0]["hooks"][0]["async"]
            .as_bool()
            .unwrap());
    }

    #[test]
    fn claude_hooks_are_idempotent() {
        let tmp = TempDir::new().unwrap();
        let first = plan_claude_hooks_change(tmp.path()).unwrap();
        write_mcp_change(&first).unwrap();
        let second = plan_claude_hooks_change(tmp.path()).unwrap();
        assert_eq!(second.action, ChangeAction::Skip);
    }
}
