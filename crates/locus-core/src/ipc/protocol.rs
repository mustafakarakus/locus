//! Versioned, local-only IPC request/response protocol.
//!
//! The wire format is newline-delimited JSON. Every request carries a protocol
//! version, a caller-supplied request id, a command name, and a free-form
//! payload. Successful responses carry an optional payload plus a `warnings`
//! array; failures carry a structured `error`. See U-006 for the full contract.

use serde::{Deserialize, Serialize};

use crate::search::Hit;

/// Current IPC protocol version. Requests declaring a different version are
/// rejected with [`error_code::UNSUPPORTED_VERSION`].
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum accepted size (in bytes) of a single newline-delimited message.
/// Larger messages are rejected to bound memory use.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// Maximum number of warnings retained on a response (see D-6).
pub const MAX_WARNINGS: usize = 5;

/// Stable, machine-readable error codes returned in [`ResponseError::code`].
pub mod error_code {
    pub const INVALID_INPUT: &str = "invalid_input";
    pub const NOT_FOUND: &str = "not_found";
    pub const UNKNOWN_COMMAND: &str = "unknown_command";
    pub const UNSUPPORTED_VERSION: &str = "unsupported_version";
    pub const MESSAGE_TOO_LARGE: &str = "message_too_large";
    pub const MALFORMED_JSON: &str = "malformed_json";
    pub const INTERNAL: &str = "internal";
    pub const TIMEOUT: &str = "timeout";
}

/// Supported IPC command names.
pub mod command {
    pub const PING: &str = "ping";
    pub const STATUS: &str = "status";
    pub const REMEMBER: &str = "remember";
    pub const SEARCH: &str = "search";
    pub const CONTEXT: &str = "context";
    pub const FORGET: &str = "forget";
    pub const REINDEX: &str = "reindex";
    pub const STOP: &str = "stop";
    pub const CONFLICTS: &str = "conflicts";
}

/// A single IPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    pub id: String,
    pub cmd: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl Request {
    /// Builds a request at the current protocol version.
    pub fn new(id: impl Into<String>, cmd: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id: id.into(),
            cmd: cmd.into(),
            payload,
        }
    }
}

/// A non-fatal warning attached to an otherwise successful response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Warning {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

/// A fatal, structured error.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResponseError {
    pub code: String,
    pub message: String,
}

/// A single IPC response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub v: u32,
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

impl Response {
    /// Builds a successful response, capping and de-duplicating warnings.
    pub fn ok(id: impl Into<String>, payload: serde_json::Value, warnings: Vec<Warning>) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id: id.into(),
            ok: true,
            payload: Some(payload),
            warnings: cap_warnings(warnings),
            error: None,
        }
    }

    /// Builds a fatal error response.
    pub fn error(
        id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id: id.into(),
            ok: false,
            payload: None,
            warnings: Vec::new(),
            error: Some(ResponseError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

/// Caps warnings at [`MAX_WARNINGS`], keeping the first occurrence of each code.
pub fn cap_warnings(warnings: Vec<Warning>) -> Vec<Warning> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for warning in warnings {
        if seen.iter().any(|code| code == &warning.code) {
            continue;
        }
        seen.push(warning.code.clone());
        out.push(warning);
        if out.len() >= MAX_WARNINGS {
            break;
        }
    }
    out
}

// --- Typed command payloads -------------------------------------------------

/// Payload for the `remember` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RememberRequest {
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(rename = "type")]
    pub memory_type: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default = "default_importance")]
    pub importance: u8,
    #[serde(default)]
    pub source: Option<String>,
}

fn default_importance() -> u8 {
    50
}

/// Response payload for the `remember` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RememberResponse {
    pub id: String,
}

/// Payload for the `search` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub text: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default, rename = "type")]
    pub memory_type: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

/// Response payload for the `search` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub count: usize,
    pub hits: Vec<Hit>,
}

/// Payload for the `context` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRequest {
    pub text: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default, rename = "type")]
    pub memory_type: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub token_budget: Option<usize>,
}

/// Response payload for the `context` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextResponse {
    pub brief: String,
}

/// Payload for the `forget` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgetRequest {
    pub id: String,
}

/// Response payload for the `forget` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgetResponse {
    pub id: String,
}

/// Response payload for the `reindex` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReindexResponse {
    pub reindexed: usize,
}

/// Payload for the `conflicts` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictsRequest {
    #[serde(default)]
    pub namespace: Option<String>,
}

/// Response payload for the `conflicts` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictsResponse {
    pub count: usize,
    pub conflicts: Vec<crate::conflict::ConflictRecord>,
}

/// Response payload for the `ping` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingResponse {
    pub version: String,
    pub protocol: u32,
}

/// Response payload for the `status` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    pub version: String,
    pub protocol: u32,
    pub transport: String,
    pub endpoint: String,
    pub database: String,
    pub search_backend: String,
    pub pid: u32,
    pub uptime_seconds: u64,
    pub idle_timeout_seconds: u64,
    pub memory_count: usize,
    pub fts_row_count: usize,
    pub fts_consistent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_are_capped_and_deduped() {
        let warnings = vec![
            Warning {
                code: "a".into(),
                message: "1".into(),
                field: None,
            },
            Warning {
                code: "a".into(),
                message: "dup".into(),
                field: None,
            },
            Warning {
                code: "b".into(),
                message: "2".into(),
                field: None,
            },
            Warning {
                code: "c".into(),
                message: "3".into(),
                field: None,
            },
            Warning {
                code: "d".into(),
                message: "4".into(),
                field: None,
            },
            Warning {
                code: "e".into(),
                message: "5".into(),
                field: None,
            },
            Warning {
                code: "f".into(),
                message: "6".into(),
                field: None,
            },
        ];
        let capped = cap_warnings(warnings);
        assert_eq!(capped.len(), MAX_WARNINGS);
        assert_eq!(capped[0].message, "1");
        assert!(capped.iter().filter(|w| w.code == "a").count() == 1);
    }

    #[test]
    fn success_response_serializes_without_error_field() {
        let resp = Response::ok("r1", serde_json::json!({"k": 1}), Vec::new());
        let text = serde_json::to_string(&resp).unwrap();
        assert!(text.contains("\"ok\":true"));
        assert!(!text.contains("\"error\""));
        assert!(!text.contains("\"warnings\""));
    }

    #[test]
    fn error_response_shape_is_stable() {
        let resp = Response::error("r2", error_code::INVALID_INPUT, "bad");
        let text = serde_json::to_string(&resp).unwrap();
        assert!(text.contains("\"ok\":false"));
        assert!(text.contains("\"code\":\"invalid_input\""));
        assert!(!text.contains("\"payload\""));
    }
}
