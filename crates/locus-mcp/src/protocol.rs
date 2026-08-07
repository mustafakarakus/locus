//! Minimal MCP / JSON-RPC 2.0 types for a tools-only stdio server.
//!
//! Wire format: newline-delimited JSON (no embedded newlines). See the MCP
//! specification for lifecycle (`initialize` → `notifications/initialized`)
//! and tools (`tools/list`, `tools/call`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol version we prefer to speak when the client offers it.
pub const PREFERRED_PROTOCOL_VERSION: &str = "2025-03-26";

/// Older protocol versions we still accept.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-03-26", "2024-11-05"];

/// JSON-RPC / MCP error codes we emit.
pub mod error_code {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
}

/// A single inbound JSON-RPC message (request or notification).
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// A JSON-RPC success or error response.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn result(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// One text content block in a tool result.
#[derive(Debug, Clone, Serialize)]
pub struct TextContent {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
}

impl TextContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: "text",
            text: text.into(),
        }
    }
}

/// Result of `tools/call`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    pub content: Vec<TextContent>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}

impl CallToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![TextContent::text(text)],
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![TextContent::text(text)],
            is_error: true,
        }
    }

    /// Appends machine-readable warnings (D-6) so agents can surface them.
    pub fn with_warnings(mut self, warnings: &[locus_core::ipc::Warning]) -> Self {
        if warnings.is_empty() {
            return self;
        }
        let payload = serde_json::json!({ "warnings": warnings });
        self.content.push(TextContent::text(payload.to_string()));
        self
    }
}

/// Tool descriptor returned by `tools/list`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Negotiates a protocol version: echo the client's if we support it, else our preferred.
pub fn negotiate_version(requested: &str) -> &'static str {
    for version in SUPPORTED_PROTOCOL_VERSIONS {
        if *version == requested {
            return version;
        }
    }
    PREFERRED_PROTOCOL_VERSION
}
