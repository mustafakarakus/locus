//! End-to-end cross-agent verification (U-013).
//!
//! Simulates two agents using separate MCP server instances sharing the same
//! data directory so Agent A's saved decision is retrievable by Agent B.
//!
//! Scenarios:
//! 1. Agent A saves a decision → Agent B retrieves it
//! 2. Unrelated query returns NO_RELEVANT_MEMORY
//! 3. Project namespace prevents leakage
//! 4. Secret-like input is not stored raw
//! 5. Output stays under token budget
//! 6. MCP contract remains stable
//! 7. Conflict handling does not break retrieval

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use locus_core::ipc::paths::Paths;
use locus_core::ipc::DaemonClient;
use serde_json::{json, Value};
use tempfile::TempDir;

fn workspace_bin(name: &str) -> PathBuf {
    let mcp = PathBuf::from(env!("CARGO_BIN_EXE_locus-mcp"));
    let candidate = mcp.with_file_name(name);
    assert!(
        candidate.exists(),
        "expected {} next to locus-mcp at {}",
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

/// A shared data directory backed by a temp dir.
struct SharedDataDir {
    dir: TempDir,
}

impl SharedDataDir {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

/// A running MCP server process.
struct McpServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    paths: Paths,
    next_id: i64,
}

impl McpServer {
    /// Spawns `locus-mcp --data-dir <dir>` with an explicit `locusd` for auto-start.
    fn start_with_data_dir(shared: &SharedDataDir) -> Self {
        let paths = Paths::from_data_dir(shared.path());

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus-mcp"));
        cmd.arg("--data-dir")
            .arg(shared.path())
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
                "clientInfo": { "name": "locus-e2e", "version": "0.0.0" }
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

    #[allow(dead_code)]
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

// ---------------------------------------------------------------------------
// Scenario 1: Agent A saves a decision → Agent B retrieves it
// ---------------------------------------------------------------------------

#[test]
fn saved_decision_is_retrievable_by_second_agent() {
    let shared = SharedDataDir::new();

    // Agent A saves a decision
    let mut agent_a = McpServer::start_with_data_dir(&shared);
    let save = agent_a.call_tool(
        "memory_save",
        json!({
            "content": "Use Postgres for auth service",
            "type": "decision",
            "namespace": "project:auth",
            "title": "Auth database"
        }),
    );
    assert!(
        !McpServer::is_tool_error(&save),
        "Agent A save failed: {}",
        save
    );
    let save_text = McpServer::tool_text(&save);
    assert!(
        save_text.contains("Remembered"),
        "Expected 'Remembered', got: {save_text}"
    );
    assert!(
        save_text.contains("ID:"),
        "Expected ID in save response, got: {save_text}"
    );

    // Agent B starts later and searches
    let mut agent_b = McpServer::start_with_data_dir(&shared);
    let search = agent_b.call_tool(
        "memory_search",
        json!({
            "query": "auth database Postgres",
            "namespace": "project:auth"
        }),
    );
    assert!(
        !McpServer::is_tool_error(&search),
        "Agent B search failed: {search}"
    );
    let brief = McpServer::tool_text(&search);
    assert!(
        brief.contains("Postgres")
            || brief.contains("Auth database")
            || brief.contains("Decisions"),
        "Expected brief content, got: {brief}"
    );
    assert!(
        !brief.contains("NO_RELEVANT_MEMORY"),
        "Expected non-empty brief, got: {brief}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 2: Unrelated query returns NO_RELEVANT_MEMORY
// ---------------------------------------------------------------------------

#[test]
fn unrelated_query_returns_no_relevant_memory() {
    let shared = SharedDataDir::new();

    // Agent A saves a decision
    let mut agent_a = McpServer::start_with_data_dir(&shared);
    agent_a.call_tool(
        "memory_save",
        json!({
            "content": "Use Postgres for auth service",
            "type": "decision",
            "namespace": "project:auth",
            "title": "Auth database"
        }),
    );

    // Agent B searches for something unrelated
    let mut agent_b = McpServer::start_with_data_dir(&shared);
    let search = agent_b.call_tool(
        "memory_search",
        json!({
            "query": "kubernetes deployment yaml",
            "namespace": "project:auth"
        }),
    );
    assert!(
        !McpServer::is_tool_error(&search),
        "Search failed: {search}"
    );
    let brief = McpServer::tool_text(&search);
    assert!(
        brief.contains("NO_RELEVANT_MEMORY"),
        "Expected NO_RELEVANT_MEMORY, got: {brief}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 3: Project namespace prevents leakage
// ---------------------------------------------------------------------------

#[test]
fn project_namespace_prevents_leakage() {
    let shared = SharedDataDir::new();

    // Agent A saves a decision in project:auth
    let mut agent_a = McpServer::start_with_data_dir(&shared);
    agent_a.call_tool(
        "memory_save",
        json!({
            "content": "Use Postgres for auth service",
            "type": "decision",
            "namespace": "project:auth",
            "title": "Auth database"
        }),
    );

    // Agent B searches in project:deploy — should not find auth decision
    let mut agent_b = McpServer::start_with_data_dir(&shared);
    let search = agent_b.call_tool(
        "memory_search",
        json!({
            "query": "auth database Postgres",
            "namespace": "project:deploy"
        }),
    );
    assert!(
        !McpServer::is_tool_error(&search),
        "Search failed: {search}"
    );
    let brief = McpServer::tool_text(&search);
    assert!(
        brief.contains("NO_RELEVANT_MEMORY"),
        "Expected NO_RELEVANT_MEMORY due to namespace isolation, got: {brief}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 4: Secret-like input is not stored raw
// ---------------------------------------------------------------------------

#[test]
fn secret_like_input_is_not_stored_raw() {
    let shared = SharedDataDir::new();

    // Agent A saves a memory with a secret-like pattern (AWS key)
    let mut agent_a = McpServer::start_with_data_dir(&shared);
    let save = agent_a.call_tool(
        "memory_save",
        json!({
            "content": "Use AWS key AKIAIOSFODNN7EXAMPLE for auth",
            "type": "fact",
            "namespace": "global",
            "title": "AWS credentials"
        }),
    );
    assert!(!McpServer::is_tool_error(&save), "Save failed: {save}");
    let save_text = McpServer::tool_text(&save);
    assert!(
        save_text.contains("WARNING") || save_text.contains("redact"),
        "Expected warning about secret, got: {save_text}"
    );
    assert!(
        !save_text.contains("AKIAIOSFODNN7EXAMPLE"),
        "Secret pattern should be redacted in response, got: {save_text}"
    );

    // Agent B searches and verifies the secret is not stored raw
    let mut agent_b = McpServer::start_with_data_dir(&shared);
    let search = agent_b.call_tool(
        "memory_search",
        json!({
            "query": "AWS key AKIAIOSFODNN7EXAMPLE",
            "namespace": "global"
        }),
    );
    assert!(
        !McpServer::is_tool_error(&search),
        "Search failed: {search}"
    );
    let brief = McpServer::tool_text(&search);
    assert!(
        !brief.contains("AKIAIOSFODNN7EXAMPLE"),
        "Secret should not appear in search results, got: {brief}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 5: Output stays under token budget
// ---------------------------------------------------------------------------

#[test]
fn output_stays_under_token_budget() {
    let shared = SharedDataDir::new();

    // Agent A saves multiple decisions to create a richer brief
    let mut agent_a = McpServer::start_with_data_dir(&shared);
    let decisions = vec![
        ("Use Postgres for auth service", "Auth database"),
        ("Use Redis for session storage", "Session storage"),
        ("Use JWT for token auth", "Token format"),
    ];
    for (content, title) in decisions {
        let save = agent_a.call_tool(
            "memory_save",
            json!({
                "content": content,
                "type": "decision",
                "namespace": "project:auth",
                "title": title
            }),
        );
        assert!(!McpServer::is_tool_error(&save), "Save failed: {save}");
    }

    // Agent B searches
    let mut agent_b = McpServer::start_with_data_dir(&shared);
    let search = agent_b.call_tool(
        "memory_search",
        json!({
            "query": "auth service",
            "namespace": "project:auth"
        }),
    );
    assert!(
        !McpServer::is_tool_error(&search),
        "Search failed: {search}"
    );
    let brief = McpServer::tool_text(&search);

    // Token budget: under 400 tokens (rough estimate: ~4 chars per token)
    let token_count = brief.len() / 4;
    assert!(
        token_count < 400,
        "Brief {} chars (~{} tokens) exceeds 400 token budget: {brief}",
        brief.len(),
        token_count
    );
}

// ---------------------------------------------------------------------------
// Scenario 6: MCP contract remains stable
// ---------------------------------------------------------------------------

#[test]
fn mcp_contract_remains_stable() {
    let shared = SharedDataDir::new();

    let mut server = McpServer::start_with_data_dir(&shared);

    // Locus must be reachable over the loopback Unix socket — never TCP.
    assert_eq!(
        server.paths.endpoint().transport(),
        "unix-socket",
        "daemon transport must be a Unix domain socket, not a network port"
    );

    // Verify tools list
    let id = server.next_id;
    server.next_id += 1;
    let response = server.request(id, "tools/list", json!({}));
    let tools = response["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"memory_search"));
    assert!(names.contains(&"memory_save"));
    assert!(names.contains(&"memory_forget"));
    assert!(names.contains(&"memory_status"));

    // Verify memory_save contract
    let save = server.call_tool(
        "memory_save",
        json!({
            "content": "Use Postgres for auth service",
            "type": "decision",
            "namespace": "project:auth",
            "title": "Auth database"
        }),
    );
    assert!(!McpServer::is_tool_error(&save), "Save failed: {save}");
    let save_text = McpServer::tool_text(&save);
    assert!(save_text.contains("Remembered"));
    assert!(save_text.contains("ID:"));

    // Verify memory_search contract (returns brief)
    let search = server.call_tool(
        "memory_search",
        json!({
            "query": "auth database",
            "namespace": "project:auth"
        }),
    );
    assert!(
        !McpServer::is_tool_error(&search),
        "Search failed: {search}"
    );
    let brief = McpServer::tool_text(&search);
    assert!(!brief.contains("NO_RELEVANT_MEMORY"));
    assert!(brief.contains("Decisions") || brief.contains("Postgres"));

    // Verify memory_forget contract
    let id = save_text
        .lines()
        .find_map(|line| line.strip_prefix("ID: "))
        .expect("id line")
        .trim()
        .to_string();
    let forget = server.call_tool("memory_forget", json!({ "id": id }));
    assert!(
        !McpServer::is_tool_error(&forget),
        "Forget failed: {forget}"
    );
    let forget_text = McpServer::tool_text(&forget);
    assert!(forget_text.contains("Forgot"));

    // Verify memory_status contract
    let status = server.call_tool("memory_status", json!({}));
    assert!(
        !McpServer::is_tool_error(&status),
        "Status failed: {status}"
    );
    let status_text = McpServer::tool_text(&status);
    assert!(status_text.contains("Locus status"));
}

// ---------------------------------------------------------------------------
// Scenario 7: Conflict handling does not break retrieval
// ---------------------------------------------------------------------------

#[test]
fn conflict_handling_does_not_break_retrieval() {
    let shared = SharedDataDir::new();

    // Agent A records two decisions that the daemon detects as conflicting:
    // same namespace + type and titles sharing significant keywords.
    let mut agent_a = McpServer::start_with_data_dir(&shared);
    for content in [
        "Use Postgres for auth service",
        "Use MySQL for auth service",
    ] {
        let save = agent_a.call_tool(
            "memory_save",
            json!({
                "content": content,
                "type": "decision",
                "namespace": "project:auth",
                "title": "Auth database"
            }),
        );
        assert!(!McpServer::is_tool_error(&save), "Save failed: {save}");
    }

    // The conflict must have been recorded…
    let store = locus_core::store::Store::open_at(agent_a.paths.db_file()).expect("open store");
    let conflicts = store.list_conflicts(None).expect("list conflicts");
    assert_eq!(
        conflicts.len(),
        1,
        "expected one conflict record between the two auth-database decisions"
    );

    // …without breaking retrieval: Agent B still gets a brief.
    let mut agent_b = McpServer::start_with_data_dir(&shared);
    let search = agent_b.call_tool(
        "memory_search",
        json!({
            "query": "auth database",
            "namespace": "project:auth"
        }),
    );
    assert!(
        !McpServer::is_tool_error(&search),
        "Search failed: {search}"
    );
    let brief = McpServer::tool_text(&search);
    assert!(
        !brief.contains("NO_RELEVANT_MEMORY"),
        "Expected a brief despite the conflict, got: {brief}"
    );
}
