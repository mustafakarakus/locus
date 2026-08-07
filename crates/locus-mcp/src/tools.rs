//! MCP tool definitions and handlers that forward to `locusd` over IPC.

use std::path::PathBuf;

use locus_core::ipc::paths::Paths;
use locus_core::ipc::protocol::{
    command, ContextRequest, ContextResponse, ForgetRequest, ForgetResponse, RememberRequest,
    RememberResponse, Request, StatusResponse,
};
use locus_core::ipc::{DaemonClient, Warning};
use locus_core::memory::MemoryType;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::protocol::{CallToolResult, Tool};
use crate::LOCUSD_BIN_ENV;

/// Names of tools this server exposes.
pub mod name {
    pub const SEARCH: &str = "memory_search";
    pub const SAVE: &str = "memory_save";
    pub const FORGET: &str = "memory_forget";
    pub const STATUS: &str = "memory_status";
}

/// Static tool catalog for `tools/list`.
pub fn catalog() -> Vec<Tool> {
    vec![
        Tool {
            name: name::SEARCH.into(),
            description: "Search Locus memory and return a compressed Markdown brief \
                (or NO_RELEVANT_MEMORY). Prefer this before non-trivial code changes."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (terms, identifiers, decisions)."
                    },
                    "namespace": {
                        "type": "string",
                        "description": "Optional namespace filter (e.g. project:auth)."
                    },
                    "type": {
                        "type": "string",
                        "description": "Optional memory type filter.",
                        "enum": [
                            "fact", "decision", "preference", "task",
                            "bug", "architecture", "code", "note"
                        ]
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum memories to consider (default 5).",
                        "minimum": 1,
                        "maximum": 50
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        },
        Tool {
            name: name::SAVE.into(),
            description: "Store a new memory (decision, preference, fact, etc.). \
                Do not save secrets."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "Memory body."
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional title (defaults to first line of content)."
                    },
                    "type": {
                        "type": "string",
                        "description": "Memory type (default: fact).",
                        "enum": [
                            "fact", "decision", "preference", "task",
                            "bug", "architecture", "code", "note"
                        ]
                    },
                    "namespace": {
                        "type": "string",
                        "description": "Namespace (default: global)."
                    },
                    "importance": {
                        "type": "integer",
                        "description": "Importance 0–100 (default 50).",
                        "minimum": 0,
                        "maximum": 100
                    },
                    "entities": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional related entity names."
                    },
                    "source": {
                        "type": "string",
                        "description": "Optional provenance string."
                    }
                },
                "required": ["content"],
                "additionalProperties": false
            }),
        },
        Tool {
            name: name::FORGET.into(),
            description: "Delete a memory by ID.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Memory ID returned by memory_save or search."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        },
        Tool {
            name: name::STATUS.into(),
            description: "Report Locus daemon, database, and search engine status.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
    ]
}

/// Bridges MCP tool calls to the local daemon.
pub struct ToolHost {
    paths: Paths,
    client: DaemonClient,
    locusd_bin: PathBuf,
}

impl ToolHost {
    pub fn new(paths: Paths, locusd_bin: PathBuf) -> Self {
        let client = DaemonClient::new(paths.endpoint().clone());
        Self {
            paths,
            client,
            locusd_bin,
        }
    }

    /// Ensures `locusd` is reachable, auto-starting it when needed.
    pub fn ensure_daemon(&self) -> Result<(), String> {
        self.paths
            .ensure_dirs()
            .map_err(|err| format!("failed to prepare data directory: {err}"))?;
        self.client
            .connect_or_spawn(&self.locusd_bin, self.paths.data_dir())
            .map_err(|err| err.to_string())
    }

    pub fn call(&self, tool_name: &str, arguments: Option<&Value>) -> CallToolResult {
        match tool_name {
            name::SEARCH => self.memory_search(arguments),
            name::SAVE => self.memory_save(arguments),
            name::FORGET => self.memory_forget(arguments),
            name::STATUS => self.memory_status(arguments),
            other => CallToolResult::error(format!("unknown tool: {other}")),
        }
    }

    fn memory_search(&self, arguments: Option<&Value>) -> CallToolResult {
        if let Err(err) = self.ensure_daemon() {
            return CallToolResult::error(err);
        }

        let args = arguments.unwrap_or(&Value::Null);
        let query = match required_string(args, "query") {
            Ok(q) => q,
            Err(err) => return CallToolResult::error(err),
        };
        if query.trim().is_empty() {
            return CallToolResult::error("query must not be empty");
        }

        let namespace = optional_string(args, "namespace");
        let memory_type = match optional_memory_type(args) {
            Ok(t) => t,
            Err(err) => return CallToolResult::error(err),
        };
        let limit = match optional_usize(args, "limit", 5) {
            Ok(n) => n,
            Err(err) => return CallToolResult::error(err),
        };

        let payload = ContextRequest {
            text: query,
            namespace,
            memory_type,
            limit,
            token_budget: None,
        };
        match self.ipc(command::CONTEXT, payload) {
            Ok((ContextResponse { brief }, warnings)) => {
                CallToolResult::text(brief).with_warnings(&warnings)
            }
            Err(err) => CallToolResult::error(err),
        }
    }

    fn memory_save(&self, arguments: Option<&Value>) -> CallToolResult {
        if let Err(err) = self.ensure_daemon() {
            return CallToolResult::error(err);
        }

        let args = arguments.unwrap_or(&Value::Null);
        let content = match required_string(args, "content") {
            Ok(c) => c,
            Err(err) => return CallToolResult::error(err),
        };
        if content.trim().is_empty() {
            return CallToolResult::error("content must not be empty");
        }

        let memory_type = match optional_string(args, "type") {
            Some(raw) => match MemoryType::parse(&raw) {
                Ok(t) => t.as_str().to_string(),
                Err(_) => {
                    return CallToolResult::error(format!("invalid memory type: {raw}"));
                }
            },
            None => MemoryType::Fact.as_str().to_string(),
        };

        let title = optional_string(args, "title").unwrap_or_else(|| {
            content
                .lines()
                .next()
                .unwrap_or("Untitled")
                .trim()
                .to_string()
        });

        let importance = match optional_u8(args, "importance", 50) {
            Ok(n) => n,
            Err(err) => return CallToolResult::error(err),
        };

        let entities = match args.get("entities") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    match item.as_str() {
                        Some(s) => out.push(s.to_string()),
                        None => {
                            return CallToolResult::error("entities must be an array of strings")
                        }
                    }
                }
                out
            }
            Some(_) => return CallToolResult::error("entities must be an array of strings"),
        };

        let payload = RememberRequest {
            namespace: optional_string(args, "namespace"),
            memory_type,
            title: title.clone(),
            content,
            entities,
            importance,
            source: optional_string(args, "source"),
        };

        match self.ipc(command::REMEMBER, payload) {
            Ok((RememberResponse { id }, warnings)) => {
                CallToolResult::text(format!("Remembered.\nID: {id}\nTitle: {title}"))
                    .with_warnings(&warnings)
            }
            Err(err) => CallToolResult::error(err),
        }
    }

    fn memory_forget(&self, arguments: Option<&Value>) -> CallToolResult {
        if let Err(err) = self.ensure_daemon() {
            return CallToolResult::error(err);
        }

        let args = arguments.unwrap_or(&Value::Null);
        let id = match required_string(args, "id") {
            Ok(id) => id,
            Err(err) => return CallToolResult::error(err),
        };
        if id.trim().is_empty() {
            return CallToolResult::error("id must not be empty");
        }

        let payload = ForgetRequest { id: id.clone() };
        match self.ipc(command::FORGET, payload) {
            Ok((ForgetResponse { id }, warnings)) => {
                CallToolResult::text(format!("Forgot memory {id}")).with_warnings(&warnings)
            }
            Err(err) => CallToolResult::error(err),
        }
    }

    fn memory_status(&self, arguments: Option<&Value>) -> CallToolResult {
        if let Some(args) = arguments {
            if let Some(obj) = args.as_object() {
                if !obj.is_empty() {
                    return CallToolResult::error("memory_status takes no arguments");
                }
            } else if !args.is_null() {
                return CallToolResult::error("memory_status takes no arguments");
            }
        }

        if let Err(err) = self.ensure_daemon() {
            return CallToolResult::error(err);
        }

        match self.ipc_null::<StatusResponse>(command::STATUS) {
            Ok((status, warnings)) => {
                let text = format!(
                    "Locus status\n\
                     Version: {}\n\
                     Protocol: {}\n\
                     PID: {}\n\
                     Transport: {}\n\
                     Endpoint: {}\n\
                     Database: {}\n\
                     Search: {}\n\
                     Memories: {}\n\
                     FTS rows: {} ({})\n\
                     Uptime: {}s\n\
                     Idle timeout: {}s",
                    status.version,
                    status.protocol,
                    status.pid,
                    status.transport,
                    status.endpoint,
                    status.database,
                    status.search_backend,
                    status.memory_count,
                    status.fts_row_count,
                    if status.fts_consistent {
                        "consistent"
                    } else {
                        "out of sync"
                    },
                    status.uptime_seconds,
                    status.idle_timeout_seconds,
                );
                CallToolResult::text(text).with_warnings(&warnings)
            }
            Err(err) => CallToolResult::error(err),
        }
    }

    fn ipc<T: serde::de::DeserializeOwned>(
        &self,
        cmd: &str,
        payload: impl serde::Serialize,
    ) -> Result<(T, Vec<Warning>), String> {
        let value = serde_json::to_value(payload).map_err(|err| err.to_string())?;
        let request = Request::new(Uuid::new_v4().to_string(), cmd, value);
        let response = self
            .client
            .request(&request)
            .map_err(|err| err.to_string())?;
        if !response.ok {
            let message = response
                .error
                .map(|err| format!("{}: {}", err.code, err.message))
                .unwrap_or_else(|| "daemon request failed".to_string());
            return Err(message);
        }
        let payload = response
            .payload
            .ok_or_else(|| "daemon response had no payload".to_string())?;
        let parsed: T = serde_json::from_value(payload).map_err(|err| err.to_string())?;
        Ok((parsed, response.warnings))
    }

    fn ipc_null<T: serde::de::DeserializeOwned>(
        &self,
        cmd: &str,
    ) -> Result<(T, Vec<Warning>), String> {
        let request = Request::new(Uuid::new_v4().to_string(), cmd, Value::Null);
        let response = self
            .client
            .request(&request)
            .map_err(|err| err.to_string())?;
        if !response.ok {
            let message = response
                .error
                .map(|err| format!("{}: {}", err.code, err.message))
                .unwrap_or_else(|| "daemon request failed".to_string());
            return Err(message);
        }
        let payload = response
            .payload
            .ok_or_else(|| "daemon response had no payload".to_string())?;
        let parsed: T = serde_json::from_value(payload).map_err(|err| err.to_string())?;
        Ok((parsed, response.warnings))
    }
}

/// Resolves the `locusd` binary path (env override, sibling of current exe, or PATH).
pub fn locate_daemon_binary() -> PathBuf {
    if let Ok(custom) = std::env::var(LOCUSD_BIN_ENV) {
        if !custom.trim().is_empty() {
            return PathBuf::from(custom);
        }
    }

    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            let sibling = dir.join(daemon_file_name());
            if sibling.exists() {
                return sibling;
            }
        }
    }

    PathBuf::from("locusd")
}

#[cfg(windows)]
fn daemon_file_name() -> &'static str {
    "locusd.exe"
}

#[cfg(not(windows))]
fn daemon_file_name() -> &'static str {
    "locusd"
}

fn required_string(args: &Value, key: &str) -> Result<String, String> {
    match args.get(key).and_then(Value::as_str) {
        Some(s) => Ok(s.to_string()),
        None => Err(format!("missing required string field: {key}")),
    }
}

fn optional_string(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

fn optional_memory_type(args: &Value) -> Result<Option<String>, String> {
    match optional_string(args, "type") {
        Some(raw) => MemoryType::parse(&raw)
            .map(|t| Some(t.as_str().to_string()))
            .map_err(|_| format!("invalid memory type: {raw}")),
        None => Ok(None),
    }
}

fn optional_usize(args: &Value, key: &str, default: usize) -> Result<usize, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Number(n)) => {
            let Some(v) = n.as_u64() else {
                return Err(format!("{key} must be a positive integer"));
            };
            if v == 0 || v > 50 {
                return Err(format!("{key} must be between 1 and 50"));
            }
            Ok(v as usize)
        }
        Some(_) => Err(format!("{key} must be a positive integer")),
    }
}

fn optional_u8(args: &Value, key: &str, default: u8) -> Result<u8, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Number(n)) => {
            let Some(v) = n.as_u64() else {
                return Err(format!("{key} must be an integer 0–100"));
            };
            if v > 100 {
                return Err(format!("{key} must be an integer 0–100"));
            }
            Ok(v as u8)
        }
        Some(_) => Err(format!("{key} must be an integer 0–100")),
    }
}
