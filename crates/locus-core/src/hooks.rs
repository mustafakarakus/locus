//! Host-specific lifecycle hook adapters for context injection (U-015).
//!
//! MCP is pull-based: context is only injected when the model decides to call
//! a tool. Hosts with a lifecycle hook system (Claude Code, …) can instead push
//! a [`ContextBrief`](crate::context) before the model starts reasoning. Each
//! adapter maps its host's lifecycle event payload to a single internal call:
//! inject context for a [`InjectTrigger`].
//!
//! Design constraints (U-015):
//! - host-specific adapters, not one generic hooks API
//! - injection uses the exact same brief generator as MCP (`store.context_brief`
//!   / `store.summary_brief` → `context::build_context_brief`)
//! - read-only, fast path with a small token budget

use std::path::Path;

use crate::context::{self, ContextBriefOptions};
use crate::search::Query;
use crate::store::Store;
use crate::{Error, Result};

/// Small default token budget for pre-reasoning injection (U-015).
pub const DEFAULT_INJECTION_TOKEN_BUDGET: usize = 200;

/// Strategy used when a trigger has no user query yet (session-start).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultQueryStrategy {
    /// Inject a namespace-scoped project summary plus top decisions.
    Summary,
    /// Inject nothing until the first real query.
    Skip,
}

impl DefaultQueryStrategy {
    /// Parses a strategy name (`summary` or `none`/`skip`).
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim() {
            "summary" => Ok(Self::Summary),
            "none" | "skip" => Ok(Self::Skip),
            other => Err(Error::InvalidInput(format!(
                "unknown default-query strategy: {other}"
            ))),
        }
    }
}

/// A normalized, host-agnostic request to inject context for a trigger.
///
/// `query` is `None` for session-start events; the [`DefaultQueryStrategy`]
/// decides what gets injected then. Every adapter maps its host payload onto
/// exactly this shape.
#[derive(Debug, Clone)]
pub struct InjectTrigger {
    /// Resolved namespace (e.g. `project:my-app`), derived from host payload.
    pub namespace: Option<String>,
    /// Free-form user query for pre-tool / prompt events, `None` otherwise.
    pub query: Option<String>,
    /// What to inject when `query` is `None`.
    pub strategy: DefaultQueryStrategy,
    /// Token budget for the generated brief.
    pub token_budget: usize,
}

impl InjectTrigger {
    /// Builds a trigger for a query-less event with the default strategy.
    pub fn session_start(namespace: Option<String>) -> Self {
        Self {
            namespace,
            query: None,
            strategy: DefaultQueryStrategy::Summary,
            token_budget: DEFAULT_INJECTION_TOKEN_BUDGET,
        }
    }
}

/// Contract every host adapter implements.
///
/// Adapters must be total: on an unparseable payload they return an
/// [`Error`], which callers treat as graceful degradation (never block the
/// host). They never write memory and never touch anything but the payload.
pub trait HookAdapter: Send + Sync {
    /// Stable host identifier (e.g. `claude-code`).
    fn host(&self) -> &'static str;

    /// Translate a host lifecycle hook payload into an [`InjectTrigger`].
    fn translate(&self, payload: &str) -> Result<InjectTrigger>;
}

/// Resolve an adapter by host name. New hosts are separate, explicit work.
pub fn adapter_for(host: &str) -> Result<Box<dyn HookAdapter>> {
    match host.trim() {
        "claude-code" => Ok(Box::new(ClaudeCodeAdapter)),
        other => Err(Error::InvalidInput(format!("unknown hook host: {other}"))),
    }
}

/// Adapter for Claude Code lifecycle hooks.
///
/// Reads the JSON payload Claude Code passes to a hook on stdin and maps the
/// `hook_event_name`:
/// - `SessionStart` → query-less trigger (default-query strategy)
/// - `UserPromptSubmit` → trigger using the submitted prompt
/// - `PreToolUse` → trigger using the tool name and target file path
/// - any other / unknown event → query-less trigger (graceful, no-op path)
pub struct ClaudeCodeAdapter;

impl HookAdapter for ClaudeCodeAdapter {
    fn host(&self) -> &'static str {
        "claude-code"
    }

    fn translate(&self, payload: &str) -> Result<InjectTrigger> {
        let value: serde_json::Value = serde_json::from_str(payload).map_err(|err| {
            Error::InvalidInput(format!("claude-code hook payload is not valid JSON: {err}"))
        })?;

        let namespace = value
            .get("cwd")
            .and_then(|v| v.as_str())
            .and_then(project_namespace);

        let event = value
            .get("hook_event_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let query = match event {
            "UserPromptSubmit" => value
                .get("prompt")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned),
            "PreToolUse" => Some(pre_tool_query(&value)),
            _ => None,
        };

        Ok(InjectTrigger {
            namespace,
            query,
            strategy: DefaultQueryStrategy::Summary,
            token_budget: DEFAULT_INJECTION_TOKEN_BUDGET,
        })
    }
}

fn pre_tool_query(value: &serde_json::Value) -> String {
    let tool = value
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("tool");
    let file = value
        .get("tool_input")
        .and_then(|v| v.get("file_path"))
        .and_then(|v| v.as_str());
    match file {
        Some(path) if !path.trim().is_empty() => format!("{tool} {path}"),
        _ => tool.to_string(),
    }
}

fn project_namespace(cwd: &str) -> Option<String> {
    let name = Path::new(cwd)
        .file_name()
        .and_then(|s| s.to_str())
        .map(sanitize_name)?;
    if name.is_empty() {
        return None;
    }
    Some(format!("project:{name}"))
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
    s.trim_matches('-').to_string()
}

/// Shared injection path: hook triggers and MCP both end up here.
///
/// With a query this is exactly `store.context_brief` (the MCP path), so the
/// hook and MCP output are byte-identical for the same query. Without a query
/// it applies the trigger's [`DefaultQueryStrategy`] through the same
/// `build_context_brief` engine.
pub fn inject_context(store: &Store, trigger: &InjectTrigger) -> Result<String> {
    let options = ContextBriefOptions {
        token_budget: trigger.token_budget,
    };

    match trigger.query.as_deref().map(str::trim) {
        Some(query) if !query.is_empty() => {
            let search = Query {
                text: query.to_string(),
                namespace: trigger.namespace.clone(),
                memory_type: None,
                limit: 20,
            };
            store.context_brief(search, options)
        }
        _ => match trigger.strategy {
            DefaultQueryStrategy::Summary => {
                store.summary_brief(trigger.namespace.as_deref(), options)
            }
            DefaultQueryStrategy::Skip => Ok(context::NO_RELEVANT_MEMORY.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::estimated_tokens;
    use crate::memory::{MemoryType, NewMemory};
    use tempfile::TempDir;

    fn sample_payload(event: &str) -> String {
        format!(r#"{{"session_id":"s1","cwd":"/repo/my-app","hook_event_name":"{event}"}}"#)
    }

    #[test]
    fn adapter_resolves_claude_code_only() {
        assert!(adapter_for("claude-code").is_ok());
        assert!(adapter_for("cursor").is_err());
    }

    #[test]
    fn claude_code_session_start_is_queryless_and_scoped() {
        let trigger = ClaudeCodeAdapter
            .translate(&sample_payload("SessionStart"))
            .unwrap();
        assert_eq!(trigger.query, None);
        assert_eq!(trigger.namespace.as_deref(), Some("project:my-app"));
        assert_eq!(trigger.strategy, DefaultQueryStrategy::Summary);
    }

    #[test]
    fn claude_code_prompt_submit_uses_prompt() {
        let payload = r#"{"cwd":"/repo/my-app","hook_event_name":"UserPromptSubmit","prompt":"  fix the auth bug  "}"#;
        let trigger = ClaudeCodeAdapter.translate(payload).unwrap();
        assert_eq!(trigger.query.as_deref(), Some("fix the auth bug"));
    }

    #[test]
    fn claude_code_pre_tool_uses_tool_and_file() {
        let payload = r#"{"cwd":"/repo/my-app","hook_event_name":"PreToolUse","tool_name":"Write","tool_input":{"file_path":"src/auth.rs"}}"#;
        let trigger = ClaudeCodeAdapter.translate(payload).unwrap();
        assert_eq!(trigger.query.as_deref(), Some("Write src/auth.rs"));
    }

    #[test]
    fn claude_code_pre_tool_without_file_uses_tool_only() {
        let payload = r#"{"cwd":"/repo/my-app","hook_event_name":"PreToolUse","tool_name":"Bash"}"#;
        let trigger = ClaudeCodeAdapter.translate(payload).unwrap();
        assert_eq!(trigger.query.as_deref(), Some("Bash"));
    }

    #[test]
    fn claude_code_unknown_event_defaults_to_session_start() {
        let trigger = ClaudeCodeAdapter
            .translate(&sample_payload("PostToolUse"))
            .unwrap();
        assert_eq!(trigger.query, None);
        assert_eq!(trigger.namespace.as_deref(), Some("project:my-app"));
    }

    #[test]
    fn claude_code_missing_cwd_yields_no_namespace() {
        let payload = r#"{"session_id":"s1","hook_event_name":"SessionStart"}"#;
        let trigger = ClaudeCodeAdapter.translate(payload).unwrap();
        assert_eq!(trigger.namespace, None);
    }

    #[test]
    fn claude_code_invalid_payload_is_rejected() {
        let err = ClaudeCodeAdapter
            .translate("not json")
            .expect_err("should fail");
        assert!(matches!(err, Error::InvalidInput(_)));
    }

    #[test]
    fn strategy_parse_accepts_summary_and_skip() {
        assert_eq!(
            DefaultQueryStrategy::parse("summary").unwrap(),
            DefaultQueryStrategy::Summary
        );
        assert_eq!(
            DefaultQueryStrategy::parse("none").unwrap(),
            DefaultQueryStrategy::Skip
        );
        assert!(DefaultQueryStrategy::parse("everything").is_err());
    }

    fn seed_store(path: &std::path::Path) -> Store {
        let store = Store::open_at(path).unwrap();
        let decision = NewMemory {
            namespace: Some("project:my-app".into()),
            memory_type: MemoryType::Decision,
            title: "Auth DB".into(),
            content: "Use Postgres for the auth service".into(),
            entities: vec![],
            importance: 70,
            source: None,
        };
        store.insert_memory(decision).unwrap();
        let other = NewMemory {
            namespace: Some("project:other".into()),
            memory_type: MemoryType::Fact,
            title: "Unrelated".into(),
            content: "Everything about a different project".into(),
            entities: vec![],
            importance: 50,
            source: None,
        };
        store.insert_memory(other).unwrap();
        store
    }

    #[test]
    fn hook_output_matches_mcp_brief_for_same_query() {
        let tmp = TempDir::new().unwrap();
        let store = seed_store(tmp.path().join("locus.db").as_path());

        let search = Query {
            text: "database choice".into(),
            namespace: Some("project:my-app".into()),
            memory_type: None,
            limit: 20,
        };
        let mcp_brief = store
            .context_brief(search.clone(), ContextBriefOptions::default())
            .unwrap();

        let trigger = InjectTrigger {
            namespace: Some("project:my-app".into()),
            query: Some("database choice".into()),
            strategy: DefaultQueryStrategy::Summary,
            token_budget: 400,
        };
        let hook_brief = inject_context(&store, &trigger).unwrap();
        assert_eq!(hook_brief, mcp_brief);
    }

    #[test]
    fn session_start_returns_namespace_scoped_brief() {
        let tmp = TempDir::new().unwrap();
        let store = seed_store(tmp.path().join("locus.db").as_path());

        let own = inject_context(
            &store,
            &InjectTrigger::session_start(Some("project:my-app".into())),
        )
        .unwrap();
        assert!(own.contains("Auth DB"));
        assert!(!own.contains("Unrelated"));

        let unrelated = inject_context(
            &store,
            &InjectTrigger::session_start(Some("project:other".into())),
        )
        .unwrap();
        assert!(unrelated.contains("Unrelated"));
        assert!(!unrelated.contains("Auth DB"));
    }

    #[test]
    fn unrelated_session_returns_no_relevant_memory() {
        let tmp = TempDir::new().unwrap();
        let store = seed_store(tmp.path().join("locus.db").as_path());

        let brief = inject_context(
            &store,
            &InjectTrigger::session_start(Some("project:absent".into())),
        )
        .unwrap();
        assert_eq!(brief, context::NO_RELEVANT_MEMORY);
    }

    #[test]
    fn injection_stays_under_token_budget() {
        let tmp = TempDir::new().unwrap();
        let store = seed_store(tmp.path().join("locus.db").as_path());

        let trigger = InjectTrigger {
            namespace: Some("project:my-app".into()),
            query: Some("database".into()),
            strategy: DefaultQueryStrategy::Summary,
            token_budget: 200,
        };
        let brief = inject_context(&store, &trigger).unwrap();
        assert!(estimated_tokens(&brief) <= 200);
    }

    #[test]
    fn injection_is_read_only() {
        let tmp = TempDir::new().unwrap();
        let store = seed_store(tmp.path().join("locus.db").as_path());
        let before = store.memory_count().unwrap();

        inject_context(
            &store,
            &InjectTrigger::session_start(Some("project:my-app".into())),
        )
        .unwrap();
        inject_context(
            &store,
            &InjectTrigger {
                namespace: Some("project:my-app".into()),
                query: Some("database".into()),
                strategy: DefaultQueryStrategy::Summary,
                token_budget: 200,
            },
        )
        .unwrap();

        assert_eq!(store.memory_count().unwrap(), before);
    }

    #[test]
    fn skip_strategy_injects_nothing_without_query() {
        let tmp = TempDir::new().unwrap();
        let store = seed_store(tmp.path().join("locus.db").as_path());

        let trigger = InjectTrigger {
            namespace: Some("project:my-app".into()),
            query: None,
            strategy: DefaultQueryStrategy::Skip,
            token_budget: 200,
        };
        assert_eq!(
            inject_context(&store, &trigger).unwrap(),
            context::NO_RELEVANT_MEMORY
        );
    }
}
