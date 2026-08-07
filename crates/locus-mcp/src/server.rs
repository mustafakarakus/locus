//! Blocking MCP stdio server loop.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use locus_core::ipc::paths::Paths;
use serde_json::{json, Value};

use crate::protocol::{
    error_code, negotiate_version, CallToolResult, JsonRpcRequest, JsonRpcResponse, Tool,
    PREFERRED_PROTOCOL_VERSION,
};
use crate::tools::{self, ToolHost};

/// Runtime configuration for the MCP server.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Data directory (honors `LOCUS_HOME` when left as default resolve).
    pub data_dir: Option<PathBuf>,
    /// Explicit path to `locusd` (otherwise auto-discovered).
    pub locusd_bin: Option<PathBuf>,
}

/// Runs the MCP server on stdin/stdout until stdin closes.
pub fn run_stdio(config: Config) -> Result<()> {
    let paths = match config.data_dir {
        Some(dir) => Paths::from_data_dir(dir),
        None => Paths::resolve().context("failed to resolve Locus data directory")?,
    };
    let locusd_bin = config
        .locusd_bin
        .unwrap_or_else(tools::locate_daemon_binary);
    let host = ToolHost::new(paths, locusd_bin);

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut initialized = false;

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("locus-mcp: failed reading stdin: {err}");
                break;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(req) => req,
            Err(err) => {
                // Parse errors have a null id per JSON-RPC.
                let response = JsonRpcResponse::error(
                    Value::Null,
                    error_code::PARSE_ERROR,
                    format!("parse error: {err}"),
                );
                write_response(&mut stdout, &response)?;
                continue;
            }
        };

        if request.jsonrpc != "2.0" {
            if let Some(id) = request.id.clone() {
                let response = JsonRpcResponse::error(
                    id,
                    error_code::INVALID_REQUEST,
                    "jsonrpc must be \"2.0\"",
                );
                write_response(&mut stdout, &response)?;
            }
            continue;
        }

        if let Some(response) = dispatch(&host, &request, &mut initialized) {
            write_response(&mut stdout, &response)?;
        }
    }

    Ok(())
}

fn write_response(stdout: &mut io::Stdout, response: &JsonRpcResponse) -> Result<()> {
    let mut encoded = serde_json::to_string(response)?;
    // MCP stdio forbids embedded newlines in a message.
    if encoded.contains('\n') {
        encoded = encoded.replace(['\n', '\r'], " ");
    }
    stdout.write_all(encoded.as_bytes())?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

fn dispatch(
    host: &ToolHost,
    request: &JsonRpcRequest,
    initialized: &mut bool,
) -> Option<JsonRpcResponse> {
    match request.method.as_str() {
        "initialize" => {
            let id = request.id.clone().unwrap_or(Value::Null);
            Some(handle_initialize(id, request.params.as_ref()))
        }
        "notifications/initialized" => {
            *initialized = true;
            None
        }
        "ping" => {
            let id = request.id.clone()?;
            Some(JsonRpcResponse::result(id, json!({})))
        }
        "tools/list" => {
            let id = request.id.clone()?;
            if !*initialized {
                return Some(JsonRpcResponse::error(
                    id,
                    error_code::INVALID_REQUEST,
                    "server not initialized",
                ));
            }
            Some(handle_tools_list(id))
        }
        "tools/call" => {
            let id = request.id.clone()?;
            if !*initialized {
                return Some(JsonRpcResponse::error(
                    id,
                    error_code::INVALID_REQUEST,
                    "server not initialized",
                ));
            }
            Some(handle_tools_call(host, id, request.params.as_ref()))
        }
        other if request.is_notification() => {
            // Ignore unknown notifications (e.g. cancelled) quietly.
            let _ = other;
            None
        }
        other => {
            let id = request.id.clone()?;
            Some(JsonRpcResponse::error(
                id,
                error_code::METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            ))
        }
    }
}

fn handle_initialize(id: Value, params: Option<&Value>) -> JsonRpcResponse {
    let requested = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or(PREFERRED_PROTOCOL_VERSION);
    let version = negotiate_version(requested);

    JsonRpcResponse::result(
        id,
        json!({
            "protocolVersion": version,
            "capabilities": {
                "tools": {
                    "listChanged": false
                }
            },
            "serverInfo": {
                "name": "locus",
                "version": locus_core::VERSION
            },
            "instructions": "Locus is a local-first memory layer. Before non-trivial code changes, call memory_search. When a decision is confirmed, call memory_save. Never save secrets. If memory_search returns NO_RELEVANT_MEMORY, continue normally."
        }),
    )
}

fn handle_tools_list(id: Value) -> JsonRpcResponse {
    let tools: Vec<Tool> = tools::catalog();
    JsonRpcResponse::result(id, json!({ "tools": tools }))
}

fn handle_tools_call(host: &ToolHost, id: Value, params: Option<&Value>) -> JsonRpcResponse {
    let Some(params) = params else {
        return JsonRpcResponse::error(id, error_code::INVALID_PARAMS, "params are required");
    };

    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return JsonRpcResponse::error(id, error_code::INVALID_PARAMS, "params.name is required");
    };

    let arguments = params.get("arguments");

    // Unknown tool is a protocol-level error (not a tool-result isError).
    if !matches!(
        name,
        tools::name::SEARCH | tools::name::SAVE | tools::name::FORGET | tools::name::STATUS
    ) {
        return JsonRpcResponse::error(
            id,
            error_code::METHOD_NOT_FOUND,
            format!("unknown tool: {name}"),
        );
    }

    let result: CallToolResult = host.call(name, arguments);
    match serde_json::to_value(result) {
        Ok(value) => JsonRpcResponse::result(id, value),
        Err(err) => JsonRpcResponse::error(
            id,
            error_code::INTERNAL_ERROR,
            format!("failed to serialize tool result: {err}"),
        ),
    }
}
