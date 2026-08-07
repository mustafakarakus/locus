//! Client for talking to a running `locusd` over the local IPC transport.
//!
//! The client is intentionally simple: each request opens a short-lived
//! connection, writes one newline-delimited request, and reads one response.
//! It also knows how to auto-start a daemon that isn't running yet.

use std::io::BufReader;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use interprocess::local_socket::{prelude::*, Stream};

use crate::ipc::paths::Endpoint;
use crate::ipc::protocol::{command, PingResponse, Request, Response, PROTOCOL_VERSION};
use crate::ipc::wire::{read_message, write_message, ReadOutcome, DEFAULT_MAX_MESSAGE_BYTES};
use crate::{Error, Result};

/// How long to wait for a freshly spawned daemon to become reachable.
const SPAWN_READY_TIMEOUT: Duration = Duration::from_secs(5);
const SPAWN_POLL_INTERVAL: Duration = Duration::from_millis(20);
/// Default per-request read timeout guarding against a stuck daemon.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A connection factory + request helper for a single daemon endpoint.
#[derive(Debug, Clone)]
pub struct DaemonClient {
    endpoint: Endpoint,
}

impl DaemonClient {
    /// Builds a client for the given endpoint.
    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    /// The endpoint this client targets.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    fn connect(&self) -> Result<Stream> {
        let name = self.endpoint.to_name()?;
        let stream = Stream::connect(name)?;
        stream.set_recv_timeout(Some(REQUEST_TIMEOUT))?;
        stream.set_send_timeout(Some(REQUEST_TIMEOUT))?;
        Ok(stream)
    }

    /// Sends a single request and returns the daemon's response.
    pub fn request(&self, request: &Request) -> Result<Response> {
        let stream = self.connect()?;
        let payload = serde_json::to_vec(request)?;

        let mut writer = &stream;
        write_message(&mut writer, &payload)?;

        let mut reader = BufReader::new(&stream);
        match read_message(&mut reader, DEFAULT_MAX_MESSAGE_BYTES)? {
            ReadOutcome::Message(bytes) => {
                let response: Response = serde_json::from_slice(&bytes)?;
                Ok(response)
            }
            ReadOutcome::TooLarge => Err(Error::Other(
                "daemon response exceeded the maximum message size".to_string(),
            )),
            ReadOutcome::Eof => Err(Error::Other(
                "daemon closed the connection without responding".to_string(),
            )),
        }
    }

    /// Sends a `ping` and returns the parsed liveness payload.
    pub fn ping(&self) -> Result<PingResponse> {
        let request = Request::new("ping", command::PING, serde_json::Value::Null);
        let response = self.request(&request)?;
        if !response.ok {
            let message = response
                .error
                .map(|err| err.message)
                .unwrap_or_else(|| "ping failed".to_string());
            return Err(Error::Other(message));
        }
        let payload = response
            .payload
            .ok_or_else(|| Error::Other("ping response had no payload".to_string()))?;
        Ok(serde_json::from_value(payload)?)
    }

    /// Returns `true` if a healthy daemon currently answers on the endpoint.
    pub fn is_running(&self) -> bool {
        self.ping().is_ok()
    }

    /// Ensures a daemon is running, auto-starting `locusd` if necessary.
    ///
    /// `bin` is the path to the `locusd` executable and `data_dir` is the data
    /// directory the daemon should use. Auto-start is idempotent: if a healthy
    /// daemon already answers, this returns immediately without spawning.
    pub fn connect_or_spawn(&self, bin: &Path, data_dir: &Path) -> Result<()> {
        if self.is_running() {
            return Ok(());
        }

        Command::new(bin)
            .arg("--foreground")
            .arg("--data-dir")
            .arg(data_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| {
                Error::Other(format!(
                    "failed to auto-start locusd at {}: {err}",
                    bin.display()
                ))
            })?;

        let deadline = Instant::now() + SPAWN_READY_TIMEOUT;
        while Instant::now() < deadline {
            if self.is_running() {
                return Ok(());
            }
            std::thread::sleep(SPAWN_POLL_INTERVAL);
        }

        Err(Error::Other(
            "locusd did not become reachable after auto-start".to_string(),
        ))
    }

    /// Convenience accessor for the protocol version this client speaks.
    pub fn protocol_version(&self) -> u32 {
        PROTOCOL_VERSION
    }
}
