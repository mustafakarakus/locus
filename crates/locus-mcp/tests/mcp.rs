//! Integration tests for the Locus MCP stdio server (U-007).

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use locus_core::ipc::paths::Paths;
use locus_core::ipc::DaemonClient;
use serde_json::{json, Value};
use tempfile::TempDir;

/// Resolves a workspace binary sitting next to `locus-mcp`.
fn workspace_bin(name: &str) -> PathBuf {
    let mcp = PathBuf::from(env!("CARGO_BIN_EXE_locus-mcp"));
    let candidate = mcp.with_file_name(name);
    assert!(
        candidate.exists(),
        "expected {} next to locus-mcp at {}; run `cargo build -p locusd` (or `cargo test --workspace`)",
        name,
        candidate.display()
    );
    candidate
}

fn locusd_bin() -> PathBuf {
    workspace_bin(if cfg!(windows) {
        "locusd.exe"
    } else {
        "locusd"
    })
}

/// A running MCP server process with stdio pipes, isolated under a temp home.
struct McpServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    paths: Paths,
    _dir: TempDir,
    next_id: i64,
}

impl McpServer {
    /// Spawns `locus-mcp --data-dir <tmp>` with an explicit `locusd` for auto-start.
    fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::from_data_dir(dir.path());

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus-mcp"));
        cmd.arg("--data-dir")
            .arg(dir.path())
            .arg("--locusd")
            .arg(locusd_bin())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = cmd.spawn().expect("spawn locus-mcp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));

        let mut server = McpServer {
            child,
            stdin,
            stdout,
            paths,
            _dir: dir,
            next_id: 1,
        };
        server.handshake();
        server
    }

    fn handshake(&mut self) {
        let id = self.next_id;
        self.next_id += 1;
        let response = self.request(
            id,
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "locus-test", "version": "0.0.0" }
            }),
        );
        assert_eq!(response["result"]["protocolVersion"], "2025-03-26");
        assert!(response["result"]["capabilities"]["tools"].is_object());

        self.notify("notifications/initialized", json!({}));
    }

    fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{msg}").expect("write notify");
        self.stdin.flush().expect("flush");
    }

    fn request(&mut self, id: i64, method: &str, params: Value) -> Value {
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{msg}").expect("write request");
        self.stdin.flush().expect("flush");

        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read response");
        serde_json::from_str(line.trim()).expect("parse response JSON")
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.request(
            id,
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments,
            }),
        )
    }

    fn tool_text(response: &Value) -> String {
        response["result"]["content"]
            .as_array()
            .expect("content array")
            .iter()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn is_tool_error(response: &Value) -> bool {
        response["result"]
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    fn wait_daemon(&self, timeout: Duration) {
        let client = DaemonClient::new(self.paths.endpoint().clone());
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if client.is_running() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("daemon did not become reachable within {timeout:?}");
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Best-effort: stop a daemon this test may have auto-started.
        let client = DaemonClient::new(self.paths.endpoint().clone());
        if client.is_running() {
            let request = locus_core::ipc::protocol::Request::new(
                "stop",
                locus_core::ipc::protocol::command::STOP,
                Value::Null,
            );
            let _ = client.request(&request);
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline && client.is_running() {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

#[test]
fn mcp_server_starts_and_lists_tools() {
    let mut server = McpServer::start();
    let id = server.next_id;
    server.next_id += 1;
    let response = server.request(id, "tools/list", json!({}));
    let tools = response["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"memory_search"));
    assert!(names.contains(&"memory_save"));
    assert!(names.contains(&"memory_forget"));
    assert!(names.contains(&"memory_status"));
}

#[test]
fn memory_save_search_forget_round_trip() {
    let mut server = McpServer::start();

    let save = server.call_tool(
        "memory_save",
        json!({
            "content": "Use Postgres for auth service",
            "type": "decision",
            "namespace": "project:auth",
            "title": "Auth database"
        }),
    );
    assert!(!McpServer::is_tool_error(&save), "{save}");
    let save_text = McpServer::tool_text(&save);
    assert!(save_text.contains("Remembered"), "{save_text}");
    assert!(save_text.contains("ID:"), "{save_text}");

    let search = server.call_tool(
        "memory_search",
        json!({
            "query": "auth database Postgres",
            "namespace": "project:auth"
        }),
    );
    assert!(!McpServer::is_tool_error(&search), "{search}");
    let brief = McpServer::tool_text(&search);
    assert!(
        brief.contains("Postgres")
            || brief.contains("Auth database")
            || brief.contains("Decisions"),
        "expected brief content, got: {brief}"
    );
    assert!(!brief.contains("NO_RELEVANT_MEMORY"), "{brief}");

    let id = save_text
        .lines()
        .find_map(|line| line.strip_prefix("ID: "))
        .expect("id line")
        .trim()
        .to_string();

    let forget = server.call_tool("memory_forget", json!({ "id": id }));
    assert!(!McpServer::is_tool_error(&forget), "{forget}");
    let forget_text = McpServer::tool_text(&forget);
    assert!(forget_text.contains("Forgot"), "{forget_text}");

    let search_again = server.call_tool(
        "memory_search",
        json!({
            "query": "auth database Postgres",
            "namespace": "project:auth"
        }),
    );
    let brief_again = McpServer::tool_text(&search_again);
    assert!(
        brief_again.contains("NO_RELEVANT_MEMORY"),
        "expected empty brief after forget, got: {brief_again}"
    );
}

#[test]
fn invalid_tool_input_returns_structured_error() {
    let mut server = McpServer::start();
    let response = server.call_tool("memory_save", json!({ "type": "decision" }));
    assert!(McpServer::is_tool_error(&response), "{response}");
    let text = McpServer::tool_text(&response);
    assert!(
        text.contains("missing required") || text.contains("content"),
        "{text}"
    );
}

#[test]
fn unknown_tool_returns_error() {
    let mut server = McpServer::start();
    let id = server.next_id;
    server.next_id += 1;
    let response = server.request(
        id,
        "tools/call",
        json!({
            "name": "memory_explode",
            "arguments": {}
        }),
    );
    assert!(response.get("error").is_some(), "{response}");
    assert_eq!(response["error"]["code"], -32601);
}

#[test]
fn server_shuts_down_cleanly_on_stdin_close() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut child = Command::new(env!("CARGO_BIN_EXE_locus-mcp"))
        .arg("--data-dir")
        .arg(dir.path())
        .arg("--locusd")
        .arg(locusd_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    drop(child.stdin.take());

    let status = wait_exit(&mut child, Duration::from_secs(5));
    assert!(status.success(), "expected clean exit, got {status}");
}

#[test]
fn server_works_with_auto_started_daemon() {
    let mut server = McpServer::start();
    let client = DaemonClient::new(server.paths.endpoint().clone());
    assert!(!client.is_running());

    let status = server.call_tool("memory_status", json!({}));
    assert!(!McpServer::is_tool_error(&status), "{status}");
    let text = McpServer::tool_text(&status);
    assert!(text.contains("Locus status"), "{text}");
    assert!(text.contains("Search:"), "{text}");

    server.wait_daemon(Duration::from_secs(5));
    assert!(client.is_running());
}

#[test]
fn negotiate_older_protocol_version() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut child = Command::new(env!("CARGO_BIN_EXE_locus-mcp"))
        .arg("--data-dir")
        .arg(dir.path())
        .arg("--locusd")
        .arg(locusd_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "old-client", "version": "0" }
        }
    });
    writeln!(stdin, "{init}").unwrap();
    stdin.flush().unwrap();

    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let response: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(response["result"]["protocolVersion"], "2024-11-05");

    let _ = child.kill();
    let _ = child.wait();
}

fn wait_exit(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("process did not exit within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
