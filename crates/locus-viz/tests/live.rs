//! Integration tests for `locus-viz` (U-016).
//!
//! Each test starts a real `locusd` against a temp data dir, seeds it, starts
//! `locus-viz` against the same dir, and exercises the loopback HTTP endpoints
//! (page, data, SSE) over raw TCP.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use locus_core::ipc::paths::Paths;
use locus_core::ipc::protocol::{command, MemoryEvent, MemoryEventKind, Request};
use locus_core::ipc::DaemonClient;
use tempfile::TempDir;

/// The daemon binary sits next to this crate's test binary (in the profile
/// dir), or is overridable via `LOCUSD_BIN`.
fn locate_daemon_bin() -> PathBuf {
    if let Ok(bin) = std::env::var("LOCUSD_BIN") {
        if !bin.trim().is_empty() {
            return PathBuf::from(bin);
        }
    }
    let exe = std::env::current_exe().expect("current exe");
    let mut dir = exe.parent().expect("exe dir").to_path_buf();
    if dir.file_name().map(|n| n == "deps").unwrap_or(false) {
        dir.pop();
    }
    let candidate = dir.join("locusd");
    if candidate.exists() {
        return candidate;
    }
    panic!("cannot locate the locusd binary; set LOCUSD_BIN");
}

struct TestDaemon {
    child: Child,
    client: DaemonClient,
}

impl TestDaemon {
    fn start(dir: &TempDir) -> Self {
        let paths = Paths::from_data_dir(dir.path());
        let child = Command::new(locate_daemon_bin())
            .arg("--foreground")
            .arg("--data-dir")
            .arg(dir.path())
            .arg("--no-idle-exit")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn locusd");
        let client = DaemonClient::new(paths.endpoint().clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if client.is_running() {
                return Self { child, client };
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut child = child;
        let _ = child.kill();
        let _ = child.wait();
        panic!("daemon did not start");
    }

    fn remember(&self, title: &str, content: &str) {
        let request = Request::new(
            "t",
            command::REMEMBER,
            serde_json::json!({ "type": "fact", "title": title, "content": content }),
        );
        let response = self.client.request(&request).expect("remember");
        assert!(response.ok, "seed remember failed");
    }

    fn search(&self, text: &str) {
        let request = Request::new("s", command::SEARCH, serde_json::json!({ "text": text }));
        let response = self.client.request(&request).expect("search");
        assert!(response.ok, "search failed");
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts locus-viz against `dir` and returns its URL plus the child handle.
fn start_viz(dir: &TempDir) -> (String, Child) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_locus-viz"))
        .arg("--data-dir")
        .arg(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn locus-viz");
    let mut stdout = BufReader::new(child.stdout.take().expect("viz stdout"));
    let mut url = String::new();
    let read = stdout.read_line(&mut url).expect("read viz URL");
    assert!(read > 0, "locus-viz printed no URL");
    (url.trim().to_string(), child)
}

/// Starts locus-viz with a short idle-exit timeout so tests can verify the
/// process exits when no clients remain.
fn start_viz_idle(dir: &TempDir, idle_ms: u64) -> (String, Child) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_locus-viz"))
        .arg("--data-dir")
        .arg(dir.path())
        .env("LOCUS_VIZ_IDLE_EXIT_MS", idle_ms.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn locus-viz");
    let mut stdout = BufReader::new(child.stdout.take().expect("viz stdout"));
    let mut url = String::new();
    let read = stdout.read_line(&mut url).expect("read viz URL");
    assert!(read > 0, "locus-viz printed no URL");
    (url.trim().to_string(), child)
}

/// Fetches a path over raw HTTP and returns the full response text.
fn http_get(host: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(host).expect("connect");
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .expect("write request");
    let _ = stream.flush();
    let mut reader = BufReader::new(stream);
    let mut out = String::new();
    reader.read_to_string(&mut out).expect("read response");
    out
}

/// Connects to the SSE endpoint and consumes the response headers, returning a
/// reader positioned at the event stream.
fn open_sse(host: &str) -> BufReader<TcpStream> {
    let stream = TcpStream::connect(host).expect("connect");
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    {
        let mut writer = &stream;
        write!(writer, "GET /events HTTP/1.1\r\nHost: {host}\r\n\r\n").expect("write sse request");
        writer.flush().expect("flush sse request");
    }
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        reader.read_line(&mut line).expect("read sse headers");
        if line.trim().is_empty() {
            break;
        }
        line.clear();
    }
    reader
}

fn host_from(url: &str) -> &str {
    url.trim_start_matches("http://").trim_end_matches('/')
}

#[test]
fn serves_page_data_and_events() {
    let tmp = TempDir::new().unwrap();
    let daemon = TestDaemon::start(&tmp);
    daemon.remember("Seed memory", "seeded content for the live graph");

    let (url, mut viz) = start_viz(&tmp);
    assert!(
        url.starts_with("http://127.0.0.1:"),
        "must bind to loopback only, got {url}"
    );
    let host = host_from(&url).to_string();

    // The live page wires data + events and is offline-capable.
    let page = http_get(&host, "/");
    assert!(page.contains("Locus Memory Graph"));
    assert!(page.contains("EventSource(\"/events\")"));
    assert!(!page.contains("<script src="));

    // /data serves the redacted graph snapshot with the seeded memory.
    let data = http_get(&host, "/data");
    let data_body = data.split("\r\n\r\n").nth(1).unwrap_or_default();
    assert!(
        data_body.starts_with('{'),
        "data body must be JSON: {data_body}"
    );
    assert!(
        data_body.contains("\"Seed memory\""),
        "data missing seed: {data_body}"
    );

    // /events streams a memory_searched event when the daemon surfaces a hit.
    let mut sse = open_sse(&host);
    daemon.search("seeded");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut received = false;
    while Instant::now() < deadline {
        let mut line = String::new();
        match sse.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if let Some(payload) = line.strip_prefix("data: ") {
                    let event: MemoryEvent = serde_json::from_str(payload.trim()).expect("event");
                    if event.kind == MemoryEventKind::Searched {
                        assert_eq!(event.access_delta, 1);
                        received = true;
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
    assert!(received, "no memory_searched SSE event received");

    drop(sse);
    let _ = viz.kill();
    let _ = viz.wait();
}

#[test]
fn unknown_path_returns_404() {
    let tmp = TempDir::new().unwrap();
    let _daemon = TestDaemon::start(&tmp);
    let (url, mut viz) = start_viz(&tmp);
    let host = host_from(&url).to_string();

    let response = http_get(&host, "/nope");
    assert!(
        response.starts_with("HTTP/1.1 404"),
        "unexpected response: {response}"
    );

    let _ = viz.kill();
    let _ = viz.wait();
}

#[test]
fn exits_when_no_clients_remain() {
    let tmp = TempDir::new().unwrap();
    let _daemon = TestDaemon::start(&tmp);
    let (url, mut viz) = start_viz_idle(&tmp, 800);
    let host = host_from(&url).to_string();

    // A real page load keeps the viewer alive.
    let _ = http_get(&host, "/");
    let _ = http_get(&host, "/");

    // Once every client has gone away, the process must exit on its own.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut exited = false;
    while Instant::now() < deadline {
        match viz.try_wait() {
            Ok(Some(_)) => {
                exited = true;
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break,
        }
    }
    assert!(exited, "locus-viz did not exit after clients disconnected");
}
