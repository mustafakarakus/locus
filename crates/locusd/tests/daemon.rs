//! Integration tests for the `locusd` daemon and its IPC transport (U-006).
//!
//! Each test spawns the real `locusd` binary against an isolated, temporary
//! data directory (via `--data-dir`) and talks to it over the local socket
//! using the same [`DaemonClient`] the CLI and MCP server use. The daemon is
//! always torn down in `Drop`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::Stream;
use locus_core::ipc::paths::Paths;
use locus_core::ipc::protocol::{
    command, error_code, MemoryEvent, MemoryEventKind, Request, Response, PROTOCOL_VERSION,
};
use locus_core::ipc::DaemonClient;
use locus_core::store::Store;
use tempfile::TempDir;

/// A running daemon bound to a private temp data directory.
struct TestDaemon {
    child: Child,
    paths: Paths,
    client: DaemonClient,
    _dir: TempDir,
}

impl TestDaemon {
    /// Spawns `locusd --foreground --data-dir <tmp>` plus any extra args and
    /// blocks until it answers a ping (or panics on timeout).
    fn start(extra_args: &[&str]) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::from_data_dir(dir.path());

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locusd"));
        cmd.arg("--foreground")
            .arg("--data-dir")
            .arg(dir.path())
            .args(extra_args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let child = cmd.spawn().expect("spawn locusd");
        let client = DaemonClient::new(paths.endpoint().clone());

        let daemon = TestDaemon {
            child,
            paths,
            client,
            _dir: dir,
        };
        daemon.wait_until_running(Duration::from_secs(5));
        daemon
    }

    fn wait_until_running(&self, timeout: Duration) {
        if !wait_for(timeout, || self.client.is_running()) {
            panic!("daemon did not become reachable within {timeout:?}");
        }
    }

    fn wait_until_stopped(&self, timeout: Duration) -> bool {
        wait_for(timeout, || !self.client.is_running())
    }

    /// Waits for the daemon *process* to exit without generating any IPC
    /// activity (pinging would reset the idle timer). Used for idle-shutdown.
    fn wait_until_exited(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) => {}
                Err(_) => return false,
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Sends a well-formed request and returns the parsed response.
    fn request(&self, cmd: &str, payload: serde_json::Value) -> Response {
        let request = Request::new("t", cmd, payload);
        self.client.request(&request).expect("request")
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Polls `check` until it returns true or the timeout elapses.
fn wait_for(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    check()
}

/// Opens a raw socket connection and performs a single newline-delimited
/// request/response exchange, bypassing typed helpers. Used to probe malformed
/// and oversized input handling.
fn raw_exchange(paths: &Paths, request_bytes: &[u8]) -> Option<Vec<u8>> {
    let name = paths.endpoint().to_name().ok()?;
    let stream = Stream::connect(name).ok()?;

    {
        let mut writer = &stream;
        writer.write_all(request_bytes).ok()?;
        writer.write_all(b"\n").ok()?;
        writer.flush().ok()?;
    }

    let mut reader = BufReader::new(&stream);
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).ok()?;
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

/// Opens a raw connection and writes a single newline-delimited request.
///
/// Used by the live-event tests (U-016): after the subscription ack the daemon
/// streams one `MemoryEvent` JSON line per event.
fn raw_connect(paths: &Paths, request_bytes: &[u8]) -> Stream {
    let name = paths.endpoint().to_name().expect("endpoint name");
    let stream = Stream::connect(name).expect("connect to daemon");
    let _ = stream.set_recv_timeout(Some(Duration::from_secs(5)));

    {
        let mut writer = &stream;
        writer.write_all(request_bytes).expect("write request");
        writer.write_all(b"\n").expect("write newline");
        writer.flush().expect("flush");
    }

    stream
}

/// Reads the next newline-delimited line from a raw stream, stripping the
/// newline. Returns `None` on EOF or timeout.
fn read_line<R: BufRead>(reader: &mut R) -> Option<String> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => None,
        Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_string()),
        Err(_) => None,
    }
}

// --- Lifecycle --------------------------------------------------------------

#[test]
fn starts_and_answers_ping() {
    let daemon = TestDaemon::start(&[]);
    let ping = daemon.client.ping().expect("ping");
    assert_eq!(ping.protocol, PROTOCOL_VERSION);
    assert_eq!(ping.version, locus_core::VERSION);
}

#[test]
fn stop_command_shuts_down() {
    let daemon = TestDaemon::start(&[]);
    let response = daemon.request(command::STOP, serde_json::Value::Null);
    assert!(response.ok);
    assert!(
        daemon.wait_until_stopped(Duration::from_secs(5)),
        "daemon should stop after STOP command"
    );
}

#[test]
fn idle_timeout_shuts_down() {
    let mut daemon = TestDaemon::start(&["--idle-timeout", "1"]);
    assert!(
        daemon.wait_until_exited(Duration::from_secs(6)),
        "daemon should self-exit after the idle timeout"
    );
}

#[test]
fn no_idle_exit_keeps_running() {
    let daemon = TestDaemon::start(&["--idle-timeout", "1", "--no-idle-exit"]);
    // Give it well past the idle window; it must still be alive.
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        daemon.client.is_running(),
        "daemon must not exit when idle is disabled"
    );
}

#[test]
fn double_start_is_refused() {
    let daemon = TestDaemon::start(&[]);

    let second = Command::new(env!("CARGO_BIN_EXE_locusd"))
        .arg("--foreground")
        .arg("--data-dir")
        .arg(daemon.paths.data_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn second locusd")
        .wait_with_output()
        .expect("wait second locusd");

    assert!(
        !second.status.success(),
        "a second daemon on the same data dir must refuse to start"
    );
    // Original daemon is unaffected.
    assert!(daemon.client.is_running());
}

#[test]
fn restarts_cleanly_and_client_reconnects() {
    // Start a daemon on a fixed data dir, stop it, then start a fresh one on
    // the same dir and confirm the client reconnects to the new instance.
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::from_data_dir(dir.path());
    let client = DaemonClient::new(paths.endpoint().clone());

    let spawn = || {
        Command::new(env!("CARGO_BIN_EXE_locusd"))
            .arg("--foreground")
            .arg("--data-dir")
            .arg(dir.path())
            .arg("--no-idle-exit")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn locusd")
    };

    let mut first = spawn();
    assert!(wait_for(Duration::from_secs(5), || client.is_running()));

    // Stop the first instance and wait for the endpoint to go quiet.
    let _ = client.request(&Request::new("s", command::STOP, serde_json::Value::Null));
    assert!(wait_for(Duration::from_secs(5), || !client.is_running()));
    let _ = first.wait();

    // A fresh instance comes up on the same dir and the client reconnects.
    let mut second = spawn();
    assert!(
        wait_for(Duration::from_secs(5), || client.is_running()),
        "client should reconnect after restart"
    );
    assert!(client.ping().is_ok());

    let _ = second.kill();
    let _ = second.wait();
}

#[cfg(unix)]
#[test]
fn sigterm_shuts_down_gracefully() {
    let daemon = TestDaemon::start(&[]);
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(daemon.pid().to_string())
        .status()
        .expect("send SIGTERM");
    assert!(status.success());
    assert!(
        daemon.wait_until_stopped(Duration::from_secs(5)),
        "daemon should stop on SIGTERM"
    );
}

// --- Transport --------------------------------------------------------------

#[test]
fn handles_many_sequential_connections() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    for _ in 0..25 {
        assert!(daemon.client.ping().is_ok());
    }
}

#[test]
fn oversized_message_is_rejected() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    // One byte over the protocol maximum, no newline until the end.
    let oversized = vec![b'x'; 1024 * 1024 + 1];
    let reply = raw_exchange(&daemon.paths, &oversized).expect("reply");
    let response: Response = serde_json::from_slice(&reply).expect("json");
    assert!(!response.ok);
    assert_eq!(
        response.error.expect("error").code,
        error_code::MESSAGE_TOO_LARGE
    );
    // Daemon survives and still serves.
    assert!(daemon.client.is_running());
}

#[test]
fn malformed_json_does_not_crash() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    let reply = raw_exchange(&daemon.paths, b"{not valid json").expect("reply");
    let response: Response = serde_json::from_slice(&reply).expect("json");
    assert!(!response.ok);
    assert_eq!(
        response.error.expect("error").code,
        error_code::MALFORMED_JSON
    );
    assert!(daemon.client.is_running());
}

// --- Stale state ------------------------------------------------------------

#[test]
fn stale_socket_is_reclaimed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::from_data_dir(dir.path());

    // Simulate a leftover socket file from a crashed daemon.
    let socket = paths.endpoint().socket_file().expect("unix socket path");
    std::fs::write(socket, b"stale").expect("write stale socket");

    // Seed the DB so we can prove recovery does not delete it.
    let store = locus_core::store::Store::open_at(paths.db_file()).expect("open store");
    let id = store
        .insert_memory(locus_core::memory::NewMemory {
            namespace: None,
            memory_type: locus_core::memory::MemoryType::Fact,
            title: "keep me".to_string(),
            content: "survives daemon recovery".to_string(),
            entities: vec![],
            importance: 50,
            source: None,
        })
        .expect("insert");

    let mut child = Command::new(env!("CARGO_BIN_EXE_locusd"))
        .arg("--foreground")
        .arg("--data-dir")
        .arg(dir.path())
        .arg("--no-idle-exit")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn locusd over stale socket");

    let client = DaemonClient::new(paths.endpoint().clone());
    assert!(
        wait_for(Duration::from_secs(5), || client.is_running()),
        "daemon should reclaim the stale socket and start"
    );

    // The pre-existing memory is still there — recovery did not wipe the DB.
    let survived = store
        .get_memory_by_id(&id)
        .expect("memory should survive recovery");
    assert_eq!(survived.id, id);

    let _ = child.kill();
    let _ = child.wait();
}

// --- Concurrency ------------------------------------------------------------

#[test]
fn concurrent_reads_are_served() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    // Seed a memory so searches return a hit.
    daemon.request(
        command::REMEMBER,
        serde_json::json!({ "type": "fact", "title": "seed", "content": "concurrent read target" }),
    );
    let endpoint = daemon.paths.endpoint().clone();

    // Mix concurrent ping, status, and search requests.
    let handles: Vec<_> = (0..12)
        .map(|i| {
            let client = DaemonClient::new(endpoint.clone());
            std::thread::spawn(move || {
                let cmd = match i % 3 {
                    0 => Request::new("p", command::PING, serde_json::Value::Null),
                    1 => Request::new("s", command::STATUS, serde_json::Value::Null),
                    _ => Request::new(
                        "q",
                        command::SEARCH,
                        serde_json::json!({ "text": "concurrent" }),
                    ),
                };
                client.request(&cmd).map(|r| r.ok).unwrap_or(false)
            })
        })
        .collect();

    for handle in handles {
        assert!(handle.join().expect("thread"));
    }
}

#[test]
fn concurrent_writes_are_serialized() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    let endpoint = daemon.paths.endpoint().clone();

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let client = DaemonClient::new(endpoint.clone());
            std::thread::spawn(move || {
                let payload = serde_json::json!({
                    "type": "fact",
                    "title": format!("t{i}"),
                    "content": format!("concurrent write number {i}"),
                });
                let request = Request::new("w", command::REMEMBER, payload);
                let response = client.request(&request).expect("request");
                assert!(response.ok, "write {i} should succeed");
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("thread");
    }

    // All ten writes landed and the FTS index stayed consistent.
    let status = daemon.request(command::STATUS, serde_json::Value::Null);
    let payload = status.payload.expect("status payload");
    assert_eq!(payload["memory_count"], 10);
    assert_eq!(payload["fts_consistent"], true);
}

// --- Protocol ---------------------------------------------------------------

#[test]
fn unknown_command_returns_error() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    let response = daemon.request("frobnicate", serde_json::Value::Null);
    assert!(!response.ok);
    assert_eq!(
        response.error.expect("error").code,
        error_code::UNKNOWN_COMMAND
    );
}

#[test]
fn invalid_payload_returns_error() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    // `remember` requires a title/content object; a bare number is invalid.
    let response = daemon.request(command::REMEMBER, serde_json::json!(42));
    assert!(!response.ok);
    assert_eq!(
        response.error.expect("error").code,
        error_code::INVALID_INPUT
    );
}

#[test]
fn unsupported_version_returns_error() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    let request = Request {
        v: PROTOCOL_VERSION + 99,
        id: "v".to_string(),
        cmd: command::PING.to_string(),
        payload: serde_json::Value::Null,
    };
    let response = daemon.client.request(&request).expect("request");
    assert!(!response.ok);
    assert_eq!(
        response.error.expect("error").code,
        error_code::UNSUPPORTED_VERSION
    );
}

#[test]
fn request_id_is_preserved() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    let request = Request::new("my-unique-id", command::PING, serde_json::Value::Null);
    let response = daemon.client.request(&request).expect("request");
    assert_eq!(response.id, "my-unique-id");
    assert_eq!(response.v, PROTOCOL_VERSION);
}

// --- Commands round-trip ----------------------------------------------------

#[test]
fn full_command_roundtrip() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);

    // remember
    let remembered = daemon.request(
        command::REMEMBER,
        serde_json::json!({
            "type": "decision",
            "title": "Adopt FTS5",
            "content": "Locus uses SQLite FTS5 for full-text search",
            "importance": 80,
        }),
    );
    assert!(remembered.ok);
    let id = remembered.payload.expect("payload")["id"]
        .as_str()
        .expect("id")
        .to_string();

    // search
    let searched = daemon.request(command::SEARCH, serde_json::json!({ "text": "FTS5" }));
    let payload = searched.payload.expect("search payload");
    assert_eq!(payload["count"], 1);

    // context
    let context = daemon.request(command::CONTEXT, serde_json::json!({ "text": "search" }));
    let brief = context.payload.expect("context payload")["brief"]
        .as_str()
        .expect("brief")
        .to_string();
    assert!(brief.contains("FTS5"));

    // reindex
    let reindexed = daemon.request(command::REINDEX, serde_json::Value::Null);
    assert!(reindexed.ok);

    // forget
    let forgotten = daemon.request(command::FORGET, serde_json::json!({ "id": id }));
    assert!(forgotten.ok);

    // gone
    let after = daemon.request(command::SEARCH, serde_json::json!({ "text": "FTS5" }));
    assert_eq!(after.payload.expect("payload")["count"], 0);
}

#[test]
fn capture_writes_typed_memories_through_daemon() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);

    let captured = daemon.request(
        command::CAPTURE,
        serde_json::json!({
            "namespace": "project:my-app",
            "text": "We decided to use Postgres for the auth service. Prefer table-driven tests.",
        }),
    );
    assert!(captured.ok, "capture response ok");
    let payload = captured.payload.expect("capture payload");
    assert_eq!(payload["written"], 2, "two memories written");
    assert_eq!(payload["skipped_tasks"], 0);

    // A second capture of the same summary must not duplicate memories.
    let repeat = daemon.request(
        command::CAPTURE,
        serde_json::json!({
            "namespace": "project:my-app",
            "text": "We decided to use Postgres for the auth service. Prefer table-driven tests.",
        }),
    );
    let repeat_payload = repeat.payload.expect("repeat payload");
    assert_eq!(repeat_payload["written"], 0, "no duplicates on repeat");
    assert_eq!(repeat_payload["skipped_duplicates"], 2);

    // Captured memories are retrievable through the shared search path.
    let searched = daemon.request(
        command::SEARCH,
        serde_json::json!({ "text": "Postgres auth", "namespace": "project:my-app" }),
    );
    let search_payload = searched.payload.expect("search payload");
    assert_eq!(search_payload["count"], 1);

    // The other namespace sees none of it.
    let other = daemon.request(
        command::SEARCH,
        serde_json::json!({ "text": "Postgres", "namespace": "project:other" }),
    );
    assert_eq!(other.payload.expect("other payload")["count"], 0);
}

// --- Security ---------------------------------------------------------------

#[cfg(unix)]
#[test]
fn filesystem_permissions_are_restrictive() {
    use std::os::unix::fs::PermissionsExt;

    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    // Touch the store so the DB file exists.
    daemon.request(
        command::REMEMBER,
        serde_json::json!({ "type": "fact", "title": "x", "content": "y" }),
    );

    let mode = |p: &std::path::Path| {
        std::fs::metadata(p)
            .unwrap_or_else(|e| panic!("metadata {}: {e}", p.display()))
            .permissions()
            .mode()
            & 0o777
    };

    assert_eq!(
        mode(daemon.paths.data_dir()),
        0o700,
        "data dir must be 0700"
    );
    assert_eq!(
        mode(daemon.paths.endpoint().socket_file().expect("socket")),
        0o600,
        "socket must be 0600"
    );
    assert_eq!(mode(&daemon.paths.db_file()), 0o600, "db must be 0600");
}

#[test]
fn endpoint_is_local_only() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    // The transport is a Unix domain socket / named pipe — never a TCP port.
    assert_eq!(daemon.paths.endpoint().transport(), "unix-socket");
    let socket = daemon.paths.endpoint().socket_file().expect("socket path");
    assert!(socket.starts_with(daemon.paths.data_dir()));
}

#[test]
fn remember_with_secret_redacts_and_returns_warning() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    let secret = "ghp_123456789012345678901234567890123456";

    let response = daemon.request(
        command::REMEMBER,
        serde_json::json!({
            "type": "fact",
            "title": "Deploy token",
            "content": format!("the deploy token is {secret}"),
        }),
    );
    assert!(response.ok);
    assert!(
        !response.warnings.is_empty(),
        "expected a redaction warning"
    );
    assert_eq!(response.warnings[0].code, "secret_redacted");
    assert!(
        !response.warnings[0].message.contains(secret),
        "warning must not leak the secret"
    );

    let id = response.payload.expect("payload")["id"]
        .as_str()
        .expect("id")
        .to_string();

    // Verify the stored memory is redacted, not the raw secret.
    let store = locus_core::store::Store::open_at(daemon.paths.db_file()).expect("open store");
    let inserted = store.get_memory_by_id(&id).expect("memory exists");
    assert!(
        !inserted.content.contains(secret),
        "secret must not be stored"
    );
    assert!(inserted.content.contains("[REDACTED:github-pat]"));
}

#[test]
fn remember_allow_secret_stores_verbatim_without_warning() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    let secret = "ghp_123456789012345678901234567890123456";

    let response = daemon.request(
        command::REMEMBER,
        serde_json::json!({
            "type": "fact",
            "title": "Deploy token",
            "content": format!("the deploy token is {secret}"),
            "allow_secret": true,
        }),
    );
    assert!(response.ok);
    assert!(response.warnings.is_empty(), "no warning when allowed");

    let id = response.payload.expect("payload")["id"]
        .as_str()
        .expect("id")
        .to_string();

    let store = locus_core::store::Store::open_at(daemon.paths.db_file()).expect("open store");
    let inserted = store.get_memory_by_id(&id).expect("memory exists");
    assert!(
        inserted.content.contains(secret),
        "allow_secret keeps verbatim"
    );
}

#[test]
fn remember_clean_content_has_no_warnings() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    let response = daemon.request(
        command::REMEMBER,
        serde_json::json!({
            "type": "fact",
            "title": "Adopt FTS5",
            "content": "Locus uses SQLite FTS5 for full-text search",
        }),
    );
    assert!(response.ok);
    assert!(
        response.warnings.is_empty(),
        "no warnings for clean content"
    );
}

#[test]
fn debug_logs_do_not_contain_secrets() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);
    let secret = "ghp_123456789012345678901234567890123456";

    let response = daemon.request(
        command::REMEMBER,
        serde_json::json!({
            "type": "fact",
            "title": "Deploy token",
            "content": format!("the deploy token is {secret}"),
        }),
    );
    assert!(response.ok);

    let log_file = daemon.paths.log_file();
    let log_text = std::fs::read_to_string(log_file).expect("log file readable");
    assert!(
        !log_text.contains(secret),
        "daemon log must not contain the secret"
    );
    assert!(
        !log_text.contains("Deploy token"),
        "log must not contain memory content"
    );
}

// --- Live events & access tracking (U-016) ---------------------------------

#[test]
fn events_stream_reports_created_and_searched() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);

    let request = Request::new("evt", command::EVENTS, serde_json::json!({}));
    let mut bytes = serde_json::to_vec(&request).expect("serialize events request");
    bytes.push(b'\n');
    let stream = raw_connect(&daemon.paths, &bytes);
    let mut reader = BufReader::new(&stream);

    // The subscription ack arrives first.
    let ack_line = read_line(&mut reader).expect("subscription ack");
    let ack: Response = serde_json::from_str(&ack_line).expect("ack json");
    assert!(ack.ok, "events subscription must succeed");
    assert_eq!(ack.payload.expect("ack payload")["subscribed"], true);

    // A remember produces a `memory_created` event.
    let remembered = daemon.request(
        command::REMEMBER,
        serde_json::json!({
            "type": "fact",
            "title": "Event target",
            "content": "live event streaming test memory",
        }),
    );
    assert!(remembered.ok);
    let id = remembered.payload.expect("remember payload")["id"]
        .as_str()
        .expect("id")
        .to_string();

    let created_line = read_line(&mut reader).expect("created event");
    let created: MemoryEvent = serde_json::from_str(&created_line).expect("created event json");
    assert_eq!(created.kind, MemoryEventKind::Created);
    assert_eq!(created.memory_id, id);
    assert_eq!(created.access_delta, 0);
    assert_eq!(created.title, "Event target");

    // A search produces a `memory_searched` event for the surfaced hit.
    let searched = daemon.request(command::SEARCH, serde_json::json!({ "text": "event" }));
    assert!(searched.ok);

    let searched_line = read_line(&mut reader).expect("searched event");
    let searched_event: MemoryEvent =
        serde_json::from_str(&searched_line).expect("searched event json");
    assert_eq!(searched_event.kind, MemoryEventKind::Searched);
    assert_eq!(searched_event.memory_id, id);
    assert_eq!(searched_event.access_delta, 1);
}

#[test]
fn events_stream_reports_used_on_context() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);

    daemon.request(
        command::REMEMBER,
        serde_json::json!({
            "type": "decision",
            "title": "Context event",
            "content": "context brief memory for live event test",
        }),
    );

    let request = Request::new("evt", command::EVENTS, serde_json::json!({}));
    let mut bytes = serde_json::to_vec(&request).expect("serialize events request");
    bytes.push(b'\n');
    let stream = raw_connect(&daemon.paths, &bytes);
    let mut reader = BufReader::new(&stream);

    let ack_line = read_line(&mut reader).expect("subscription ack");
    let ack: Response = serde_json::from_str(&ack_line).expect("ack json");
    assert!(ack.ok);

    let context = daemon.request(command::CONTEXT, serde_json::json!({ "text": "context" }));
    assert!(context.ok);

    let used_line = read_line(&mut reader).expect("used event");
    let used: MemoryEvent = serde_json::from_str(&used_line).expect("used event json");
    assert_eq!(used.kind, MemoryEventKind::Used);
    assert_eq!(used.title, "Context event");
    assert_eq!(used.access_delta, 1);
}

#[test]
fn search_records_access_on_surfaced_memories() {
    let daemon = TestDaemon::start(&["--no-idle-exit"]);

    let remembered = daemon.request(
        command::REMEMBER,
        serde_json::json!({
            "type": "fact",
            "title": "Access target",
            "content": "access tracking probe memory",
        }),
    );
    assert!(remembered.ok);
    let id = remembered.payload.expect("remember payload")["id"]
        .as_str()
        .expect("id")
        .to_string();

    let store = Store::open_at(daemon.paths.db_file()).expect("open store");
    assert_eq!(
        store.get_memory_by_id(&id).expect("memory").access_count,
        0,
        "a fresh memory starts with zero accesses"
    );

    // Searching surfaces the memory; the writer fire-and-forgets the bump.
    daemon.request(command::SEARCH, serde_json::json!({ "text": "access" }));

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let memory = store.get_memory_by_id(&id).expect("memory");
        if memory.access_count >= 1 {
            assert!(
                memory.last_accessed_at.is_some(),
                "last_accessed_at must be stamped on access"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "access tracking was never recorded"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // Context also surfaces the memory and bumps access.
    daemon.request(command::CONTEXT, serde_json::json!({ "text": "access" }));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let memory = store.get_memory_by_id(&id).expect("memory");
        if memory.access_count >= 2 {
            break;
        }
        assert!(Instant::now() < deadline, "context did not record access");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn hung_events_subscriber_does_not_block_requests() {
    let mut daemon = TestDaemon::start(&["--no-idle-exit"]);
    daemon.request(
        command::REMEMBER,
        serde_json::json!({ "type": "fact", "title": "probe", "content": "subscriber probe" }),
    );

    // Open an events subscription and never read from it: this simulates a
    // hung live-view client that neither drains nor disconnects.
    let request = Request::new("evt", command::EVENTS, serde_json::json!({}));
    let mut bytes = serde_json::to_vec(&request).expect("serialize events request");
    bytes.push(b'\n');
    let _stream = raw_connect(&daemon.paths, &bytes);

    // Requests must keep succeeding while the subscriber queue sits full.
    for _ in 0..64 {
        let search = daemon.request(command::SEARCH, serde_json::json!({ "text": "probe" }));
        assert!(search.ok, "search must succeed with a saturated subscriber");
        let remember = daemon.request(
            command::REMEMBER,
            serde_json::json!({ "type": "fact", "title": "more", "content": "more probe" }),
        );
        assert!(
            remember.ok,
            "remember must succeed with a saturated subscriber"
        );
    }

    // Shutdown must not block on the unread subscriber.
    daemon.request(command::STOP, serde_json::json!({}));
    assert!(
        daemon.wait_until_exited(Duration::from_secs(5)),
        "daemon failed to shut down with an unread subscriber"
    );
}
