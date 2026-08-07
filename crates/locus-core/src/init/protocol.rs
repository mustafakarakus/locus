//! Locus Memory Protocol text installed into agent rule files.

/// Inclusive start marker — used for idempotent detection.
pub const PROTOCOL_START_MARKER: &str = "<!-- LOCUS:MEMORY_PROTOCOL:START -->";

/// Inclusive end marker — used for idempotent detection.
pub const PROTOCOL_END_MARKER: &str = "<!-- LOCUS:MEMORY_PROTOCOL:END -->";

/// True when `content` already contains an installed protocol block.
pub fn protocol_is_installed(content: &str) -> bool {
    content.contains(PROTOCOL_START_MARKER)
}

/// Render the full protocol block for `project_name` (used in namespace hint).
pub fn protocol_block(project_name: &str) -> String {
    let ns = format!("project:{project_name}");
    format!(
        r#"{PROTOCOL_START_MARKER}
# Locus Memory Protocol

Locus is this project's long-term memory layer for AI coding agents.
Use it through the MCP tools: `memory_search`, `memory_save`, `memory_forget`, `memory_status`.

## Required behavior

1. **Before non-trivial code changes**, call `memory_search` with a short query about the area you are changing (identifiers, decisions, constraints).
2. **Follow** any decisions and constraints returned in the brief.
3. **If a new decision is confirmed** with the user, call `memory_save` (prefer type `decision` or `preference`).
4. **Do not save secrets** — never store API keys, passwords, tokens, private credentials, or `.env` values in Locus.
5. **If `NO_RELEVANT_MEMORY` is returned**, continue normally.

## Namespace

Prefer namespace `{ns}` for project-specific memories. Also consider `global` for cross-project preferences.

## Tools

- `memory_search` — returns a compressed Markdown brief (or `NO_RELEVANT_MEMORY`)
- `memory_save` — store a new memory
- `memory_forget` — delete a memory by id
- `memory_status` — daemon / database status
{PROTOCOL_END_MARKER}"#
    )
}

/// Append the protocol block to existing file content without duplicating it.
///
/// Returns `(new_content, changed)`.
pub fn append_protocol(existing: &str, project_name: &str) -> (String, bool) {
    if protocol_is_installed(existing) {
        return (existing.to_string(), false);
    }
    let block = protocol_block(project_name);
    let mut out = existing.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&block);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    (out, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_is_idempotent() {
        let (once, changed) = append_protocol("", "app");
        assert!(changed);
        assert!(protocol_is_installed(&once));
        let (twice, changed2) = append_protocol(&once, "app");
        assert!(!changed2);
        assert_eq!(once, twice);
    }

    #[test]
    fn append_preserves_prefix() {
        let (out, _) = append_protocol("hello\n", "app");
        assert!(out.starts_with("hello\n"));
        assert!(out.contains(PROTOCOL_START_MARKER));
    }
}
