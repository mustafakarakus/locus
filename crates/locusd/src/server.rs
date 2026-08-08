//! Daemon runtime: shared state, the accept loop, idle monitoring, and clean
//! shutdown.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::ListenerOptions;

use locus_core::events::EventBus;
use locus_core::ipc::paths::{Endpoint, Paths};
use locus_core::ipc::protocol::StatusResponse;
use locus_core::store::Store;

use crate::config::Config;
use crate::dlog::DaemonLog;
use crate::handler;
use crate::writer::{self, WriterHandle};

const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
const MIN_IDLE_TICK: Duration = Duration::from_millis(50);
/// Per-connection read timeout so idle/stuck peers don't pin a handler thread.
pub const CONNECTION_READ_TIMEOUT: Duration = Duration::from_secs(30);

struct Life {
    last_activity: Instant,
    shutdown: bool,
}

/// State shared across the accept loop, handler threads, and monitors.
pub struct Shared {
    store: Store,
    paths: Paths,
    config: Config,
    log: DaemonLog,
    writer: WriterHandle,
    events: EventBus,
    pid: u32,
    started: Instant,
    active: AtomicUsize,
    last_error: Mutex<Option<String>>,
    life: Mutex<Life>,
    cond: Condvar,
}

impl Shared {
    /// Builds shared state and spawns the single writer thread.
    pub fn new(
        store: Store,
        paths: Paths,
        config: Config,
        log: DaemonLog,
    ) -> (Arc<Self>, JoinHandle<()>) {
        let events = EventBus::default();
        let (writer, writer_join) = writer::spawn(store.clone(), events.clone());
        let shared = Arc::new(Self {
            store,
            paths,
            config,
            log,
            writer,
            events,
            pid: std::process::id(),
            started: Instant::now(),
            active: AtomicUsize::new(0),
            last_error: Mutex::new(None),
            life: Mutex::new(Life {
                last_activity: Instant::now(),
                shutdown: false,
            }),
            cond: Condvar::new(),
        });
        (shared, writer_join)
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn writer(&self) -> &WriterHandle {
        &self.writer
    }

    pub fn events(&self) -> &EventBus {
        &self.events
    }

    pub fn log(&self) -> &DaemonLog {
        &self.log
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    pub fn active_count(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    /// Marks the start of request processing.
    pub fn begin_request(&self) {
        self.active.fetch_add(1, Ordering::SeqCst);
        self.touch();
    }

    /// Marks the end of request processing.
    pub fn end_request(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.touch();
        self.cond.notify_all();
    }

    fn touch(&self) {
        if let Ok(mut life) = self.life.lock() {
            life.last_activity = Instant::now();
        }
    }

    pub fn set_last_error(&self, message: impl Into<String>) {
        if let Ok(mut guard) = self.last_error.lock() {
            *guard = Some(message.into());
        }
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|guard| guard.clone())
    }

    pub fn is_shutdown(&self) -> bool {
        self.life.lock().map(|life| life.shutdown).unwrap_or(true)
    }

    /// Requests a clean shutdown and wakes the blocking accept loop.
    pub fn request_shutdown(&self) {
        if let Ok(mut life) = self.life.lock() {
            if life.shutdown {
                return;
            }
            life.shutdown = true;
        }
        self.cond.notify_all();
        self.wake_accept();
    }

    fn wake_accept(&self) {
        if let Ok(name) = self.paths.endpoint().to_name() {
            // A throwaway connection unblocks `accept()`; errors are harmless.
            let _ = LocalSocketStream::connect(name);
        }
    }

    /// Builds a point-in-time status snapshot.
    pub fn status_snapshot(&self) -> StatusResponse {
        let memory_count = self.store.memory_count().unwrap_or(0);
        let fts_row_count = self.store.fts_row_count().unwrap_or(0);
        StatusResponse {
            version: locus_core::VERSION.to_string(),
            protocol: locus_core::ipc::protocol::PROTOCOL_VERSION,
            transport: self.paths.endpoint().transport().to_string(),
            endpoint: self.paths.endpoint().display(),
            database: self.paths.db_file().display().to_string(),
            search_backend: "fts5".to_string(),
            pid: self.pid,
            uptime_seconds: self.uptime_seconds(),
            idle_timeout_seconds: if self.config.no_idle_exit {
                0
            } else {
                self.config.idle_timeout.as_secs()
            },
            memory_count,
            fts_row_count,
            fts_consistent: memory_count == fts_row_count,
            last_error: self.last_error(),
        }
    }

    fn wait_for_drain(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if self.active_count() == 0 {
                return;
            }
            if Instant::now() >= deadline {
                return;
            }
            if let Ok(guard) = self.life.lock() {
                let _ = self.cond.wait_timeout(guard, MIN_IDLE_TICK);
            }
        }
    }

    fn idle_monitor(&self) {
        loop {
            let guard = match self.life.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            if guard.shutdown {
                return;
            }

            if self.config.no_idle_exit {
                drop(self.cond.wait(guard));
                continue;
            }

            let idle = guard.last_activity.elapsed();
            if self.active_count() == 0 && idle >= self.config.idle_timeout {
                drop(guard);
                self.log.info("idle timeout reached; shutting down");
                self.request_shutdown();
                return;
            }

            let remaining = self
                .config
                .idle_timeout
                .saturating_sub(idle)
                .max(MIN_IDLE_TICK);
            let _ = self.cond.wait_timeout(guard, remaining);
        }
    }
}

/// Binds the platform listener, cleaning up stale endpoints if it is safe.
pub fn bind(paths: &Paths) -> io::Result<LocalSocketListener> {
    match create_listener(paths.endpoint()) {
        Ok(listener) => Ok(listener),
        Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
            // A live daemon already owns this endpoint: refuse to double-start.
            if locus_core::ipc::DaemonClient::new(paths.endpoint().clone()).is_running() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "another locusd is already running for this data directory",
                ));
            }
            // Otherwise the endpoint is stale; remove the socket file and retry.
            if let Some(socket) = paths.endpoint().socket_file() {
                let _ = std::fs::remove_file(socket);
            }
            create_listener(paths.endpoint())
        }
        Err(err) => Err(err),
    }
}

fn create_listener(endpoint: &Endpoint) -> io::Result<LocalSocketListener> {
    let name = endpoint.to_name()?;
    let listener = ListenerOptions::new().name(name).create_sync()?;

    // Restrict the socket file to the owner. `ListenerOptionsExt::mode` returns
    // `Unsupported` on some platforms (e.g. macOS), so tighten permissions with
    // an explicit chmod after the socket file exists. The parent directory is
    // already created 0700, so this is defence in depth.
    #[cfg(unix)]
    if let Some(socket) = endpoint.socket_file() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600));
    }

    Ok(listener)
}

/// Runs the accept loop until shutdown, then drains and cleans up.
pub fn serve(shared: Arc<Shared>, listener: LocalSocketListener) {
    let monitor = {
        let monitor_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("locusd-idle".to_string())
            .spawn(move || monitor_shared.idle_monitor())
            .expect("failed to spawn idle monitor")
    };

    let mut handlers: Vec<JoinHandle<()>> = Vec::new();

    loop {
        let stream = match listener.accept() {
            Ok(stream) => stream,
            Err(err) => {
                shared.log.warn(&format!("accept error: {}", err.kind()));
                if shared.is_shutdown() {
                    break;
                }
                continue;
            }
        };

        if shared.is_shutdown() {
            break;
        }

        let _ = stream.set_recv_timeout(Some(CONNECTION_READ_TIMEOUT));
        let handler_shared = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("locusd-conn".to_string())
            .spawn(move || handler::handle_connection(&handler_shared, stream))
            .expect("failed to spawn connection handler");
        handlers.push(handle);

        handlers.retain(|handle| !handle.is_finished());
    }

    shared.log.info("shutting down; draining active requests");
    shared.wait_for_drain(DRAIN_TIMEOUT);
    for handle in handlers {
        let _ = handle.join();
    }
    let _ = monitor.join();
}
