//! `locus-viz` — loopback HTTP + SSE viewer for the live memory graph (U-016).
//!
//! Spawned by `locus graph --live`. Serves a self-contained HTML page over
//! loopback HTTP (`127.0.0.1` only), the current graph snapshot at `/data`, and
//! an SSE stream at `/events` that mirrors daemon live events. The daemon link
//! uses the same newline-delimited IPC as the CLI; a dedicated thread subscribes
//! to the daemon `events` command and re-broadcasts into a local
//! [`EventBus`] that each browser client drains.
//!
//! All HTTP is implemented directly on `std::net::TcpListener` + threads — no
//! async runtime — and every client write is bounded by a send timeout so a
//! hung browser tab can never stall the viewer or the daemon.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use clap::Parser;
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::Stream;

use locus_core::events::EventBus;
use locus_core::graph::{GraphRequest, DEFAULT_GRAPH_MAX_NODES};
use locus_core::ipc::paths::Paths;
use locus_core::ipc::protocol::{command, MemoryEvent, Request};
use locus_core::ipc::wire::{read_message, ReadOutcome};
use locus_core::store::Store;

/// Retry delay for reconnecting to the daemon event stream.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
/// Read timeout on the daemon event stream.
const IPC_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Per-request read timeout so idle/stuck peers don't pin a thread.
const CONNECTION_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Send timeout on SSE writes; a hung client is dropped after this.
const SSE_SEND_TIMEOUT: Duration = Duration::from_secs(5);
/// Exit after this long with zero connected clients (tab closed).
const IDLE_EXIT_TIMEOUT: Duration = Duration::from_secs(30);
/// Env override (milliseconds) so tests can exercise idle-exit quickly.
const IDLE_EXIT_ENV: &str = "LOCUS_VIZ_IDLE_EXIT_MS";
/// How often the accept loop wakes to check for idle exit.
const ACCEPT_POLL: Duration = Duration::from_millis(200);

fn idle_exit_timeout() -> Duration {
    if let Ok(ms) = std::env::var(IDLE_EXIT_ENV) {
        if let Ok(ms) = ms.parse::<u64>() {
            return Duration::from_millis(ms);
        }
    }
    IDLE_EXIT_TIMEOUT
}

#[derive(Parser, Debug)]
#[command(name = "locus-viz")]
#[command(version = locus_core::VERSION)]
#[command(about = "Loopback viewer for the live Locus memory graph")]
struct Cli {
    /// Data directory (defaults to the resolved Locus home).
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Maximum number of nodes to include in the graph.
    #[arg(long, default_value_t = DEFAULT_GRAPH_MAX_NODES)]
    max_nodes: usize,
}

fn main() -> std::io::Result<()> {
    let args = Cli::parse();
    let paths = match args.data_dir {
        Some(dir) => Paths::from_data_dir(dir),
        None => Paths::resolve().expect("resolve locus home"),
    };

    let store = Store::open_at(paths.db_file()).expect("open store");
    let events = EventBus::default();

    // Subscribe to the daemon's live events and re-broadcast locally.
    let _bridge = event_bridge(paths.clone(), events.clone());

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    // The CLI reads this line to learn the viewer URL.
    println!("http://127.0.0.1:{port}/");

    // Track live connections so the process can exit when the tab closes.
    let active_clients = Arc::new(AtomicUsize::new(0));
    let last_client_change = Arc::new(Mutex::new(Instant::now()));
    listener.set_nonblocking(true)?;

    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                active_clients.fetch_add(1, Ordering::SeqCst);
                *last_client_change.lock().unwrap() = Instant::now();
                let store = store.clone();
                let events = events.clone();
                let max_nodes = args.max_nodes;
                let active_clients = active_clients.clone();
                let last_client_change = last_client_change.clone();
                std::thread::Builder::new()
                    .name("locus-viz-conn".to_string())
                    .spawn(move || {
                        handle_connection(stream, store, events, max_nodes);
                        active_clients.fetch_sub(1, Ordering::SeqCst);
                        *last_client_change.lock().unwrap() = Instant::now();
                    })
                    .expect("spawn connection handler");
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if active_clients.load(Ordering::SeqCst) == 0
                    && last_client_change.lock().unwrap().elapsed() > idle_exit_timeout()
                {
                    break;
                }
                std::thread::sleep(ACCEPT_POLL);
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

/// Background thread that streams daemon events into the local bus, retrying
/// forever so the viewer survives daemon restarts.
fn event_bridge(paths: Paths, events: EventBus) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("locus-viz-events".to_string())
        .spawn(move || loop {
            let _ = stream_events(&paths, &events);
            std::thread::sleep(RECONNECT_DELAY);
        })
        .expect("spawn event bridge")
}

/// Opens one daemon event stream, re-broadcasting each event. Returns when the
/// stream ends (daemon closed it or it became unreachable).
fn stream_events(paths: &Paths, events: &EventBus) -> std::io::Result<()> {
    let name = paths.endpoint().to_name()?;
    let stream = Stream::connect(name)?;
    let _ = stream.set_recv_timeout(Some(IPC_READ_TIMEOUT));

    let request = Request::new("viz", command::EVENTS, serde_json::json!({}));
    let mut bytes = serde_json::to_vec(&request).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    {
        let mut writer = &stream;
        writer.write_all(&bytes)?;
        writer.flush()?;
    }

    let mut reader = BufReader::new(&stream);
    // Subscription ack (single line) then one MemoryEvent JSON line per event.
    read_ack(&mut reader)?;

    loop {
        let outcome = read_message(&mut reader, 1024 * 1024)?;
        match outcome {
            ReadOutcome::Message(line) => {
                let event: MemoryEvent = match serde_json::from_slice(&line) {
                    Ok(event) => event,
                    Err(_) => continue,
                };
                events.publish(&event);
            }
            ReadOutcome::TooLarge => continue,
            ReadOutcome::Eof => return Ok(()),
        }
    }
}

fn read_ack(reader: &mut BufReader<&Stream>) -> std::io::Result<()> {
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Err(std::io::Error::other("daemon closed the event stream"));
    }
    Ok(())
}

fn handle_connection(stream: TcpStream, store: Store, events: EventBus, max_nodes: usize) {
    // The accept loop leaves the listener (and on macOS/BSD, its accepted
    // connections) in non-blocking mode. Restore blocking I/O so response
    // writes and the SSE send timeout behave like a normal blocking socket.
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(Some(CONNECTION_READ_TIMEOUT));
    let reader = match stream.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    };
    let mut reader = BufReader::new(reader);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let request_line = request_line.trim_end();
    if request_line.is_empty() {
        return;
    }

    // Consume headers through the blank line; GET has no body.
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if line == "\r\n" || line == "\n" {
                    break;
                }
            }
            Err(_) => return,
        }
    }

    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();

    let mut stream = stream;
    match path.as_str() {
        "/" | "/index.html" => serve_text(
            &mut stream,
            "text/html; charset=utf-8",
            &locus_core::viz::live_html(),
        ),
        "/data" => {
            let request = GraphRequest {
                max_nodes,
                ..GraphRequest::default()
            };
            match store.graph(request) {
                Ok(data) => match locus_core::viz::graph_payload_json(&data) {
                    Ok(json) => serve_text(&mut stream, "application/json", &json),
                    Err(_) => serve_error(&mut stream, 500, "internal error"),
                },
                Err(_) => serve_error(&mut stream, 500, "internal error"),
            }
        }
        "/events" => serve_sse(&mut stream, &events),
        _ => serve_error(&mut stream, 404, "not found"),
    }
}

fn serve_text(stream: &mut TcpStream, content_type: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n"
    );
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

fn serve_error(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n"
    );
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

/// Streams `MemoryEvent` values as SSE `data:` lines. A hung or disconnected
/// client is dropped via the send timeout; the subscription is always released.
fn serve_sse(stream: &mut TcpStream, events: &EventBus) {
    let _ = stream.set_write_timeout(Some(SSE_SEND_TIMEOUT));
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n"
    );
    let _ = stream.flush();

    let (subscriber_id, rx) = events.subscribe();
    while let Ok(event) = rx.recv() {
        let json = serde_json::to_string(&event).unwrap_or_default();
        let payload = format!("data: {json}\n\n");
        if write!(stream, "{payload}").is_err() || stream.flush().is_err() {
            break;
        }
    }
    events.unsubscribe(subscriber_id);
}
