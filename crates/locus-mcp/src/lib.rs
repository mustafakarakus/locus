//! MCP server for Locus — JSON-RPC 2.0 over stdio, talking to `locusd` via IPC.
//!
//! Intentionally blocking and tokio-free (see DECISIONS.md D-2). Logging goes
//! to stderr only; stdout is reserved for MCP messages.

mod protocol;
mod server;
mod tools;

pub use server::{run_stdio, Config};

/// Environment variable that overrides the discovered `locusd` binary.
pub const LOCUSD_BIN_ENV: &str = "LOCUSD_BIN";
