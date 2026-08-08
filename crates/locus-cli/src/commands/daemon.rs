//! `locus daemon` — control and inspect the background `locusd` process.
//!
//! The daemon owns the single writer path to SQLite (U-006). These
//! subcommands let a human start, stop, restart, and inspect it without
//! touching the IPC wire format directly.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use locus_core::ipc::paths::Paths;
use locus_core::ipc::protocol::{command, Request, StatusResponse};
use locus_core::ipc::DaemonClient;

/// Environment variable that overrides the discovered `locusd` binary.
const LOCUSD_BIN_ENV: &str = "LOCUSD_BIN";
/// How long `stop`/`restart` waits for the daemon to actually exit.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Control the background Locus daemon (`locusd`)
#[derive(Parser, Debug)]
pub struct DaemonCmd {
    #[command(subcommand)]
    action: DaemonAction,
}

#[derive(Subcommand, Debug)]
enum DaemonAction {
    /// Show whether the daemon is running and its live status
    Status {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Start the daemon if it is not already running
    Start,
    /// Ask a running daemon to shut down
    Stop,
    /// Stop the daemon (if running) and start a fresh instance
    Restart,
}

impl DaemonCmd {
    pub fn run(self) -> Result<()> {
        let paths = Paths::resolve().context("failed to resolve Locus data directory")?;
        let client = DaemonClient::new(paths.endpoint().clone());

        match self.action {
            DaemonAction::Status { json } => status(&client, json),
            DaemonAction::Start => start(&paths, &client),
            DaemonAction::Stop => stop(&client),
            DaemonAction::Restart => restart(&paths, &client),
        }
    }
}

fn status(client: &DaemonClient, json: bool) -> Result<()> {
    if !client.is_running() {
        if json {
            println!("{{\"running\":false}}");
        } else {
            println!("locusd is not running");
        }
        return Ok(());
    }

    let request = Request::new("status", command::STATUS, serde_json::Value::Null);
    let response = client.request(&request)?;
    if !response.ok {
        let message = response
            .error
            .map(|err| err.message)
            .unwrap_or_else(|| "status request failed".to_string());
        return Err(anyhow!(message));
    }
    let payload = response
        .payload
        .ok_or_else(|| anyhow!("status response had no payload"))?;
    let status: StatusResponse = serde_json::from_value(payload)?;

    if json {
        println!("{}", serde_json::to_string(&status)?);
    } else {
        println!("locusd is running");
        println!("  Version:       {}", status.version);
        println!("  Protocol:      {}", status.protocol);
        println!("  PID:           {}", status.pid);
        println!("  Transport:     {}", status.transport);
        println!("  Endpoint:      {}", status.endpoint);
        println!("  Database:      {}", status.database);
        println!("  Search:        {}", status.search_backend);
        println!("  Memories:      {}", status.memory_count);
        println!(
            "  FTS index:     {} rows ({})",
            status.fts_row_count,
            if status.fts_consistent {
                "consistent"
            } else {
                "out of sync — run `locus reindex`"
            }
        );
        println!("  Uptime:        {}s", status.uptime_seconds);
        println!("  Idle timeout:  {}s", status.idle_timeout_seconds);
        if let Some(err) = status.last_error {
            println!("  Last error:    {err}");
        }
    }

    Ok(())
}

fn start(paths: &Paths, client: &DaemonClient) -> Result<()> {
    if client.is_running() {
        println!("locusd is already running");
        return Ok(());
    }

    let bin = locate_daemon_binary()?;
    client
        .connect_or_spawn(&bin, paths.data_dir())
        .map_err(|err| anyhow!("{err}"))?;
    println!("locusd started");
    Ok(())
}

fn stop(client: &DaemonClient) -> Result<()> {
    if !client.is_running() {
        println!("locusd is not running");
        return Ok(());
    }

    let request = Request::new("stop", command::STOP, serde_json::Value::Null);
    // The daemon may close the connection as it shuts down; treat a transport
    // error after issuing stop as a best-effort success and confirm via polling.
    let _ = client.request(&request);

    let deadline = Instant::now() + STOP_TIMEOUT;
    while Instant::now() < deadline {
        if !client.is_running() {
            println!("locusd stopped");
            return Ok(());
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    Err(anyhow!("locusd did not stop within {STOP_TIMEOUT:?}"))
}

fn restart(paths: &Paths, client: &DaemonClient) -> Result<()> {
    if client.is_running() {
        stop(client)?;
    }
    start(paths, client)
}

/// Locates the `locusd` executable.
///
/// Resolution order:
/// 1. The `LOCUSD_BIN` environment variable, if set.
/// 2. A `locusd` binary sitting next to the current `locus` executable
///    (the normal case for a co-installed pair).
/// 3. Bare `locusd`, relying on `PATH`.
pub(crate) fn locate_daemon_binary() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var(LOCUSD_BIN_ENV) {
        if !custom.trim().is_empty() {
            return Ok(PathBuf::from(custom));
        }
    }

    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            let sibling = dir.join(daemon_file_name());
            if sibling.exists() {
                return Ok(sibling);
            }
        }
    }

    Ok(PathBuf::from("locusd"))
}

#[cfg(windows)]
fn daemon_file_name() -> &'static str {
    "locusd.exe"
}

#[cfg(not(windows))]
fn daemon_file_name() -> &'static str {
    "locusd"
}
