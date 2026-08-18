//! `locusd` — the local Locus daemon.
//!
//! Keeps the SQLite store and FTS5 search warm behind a local IPC endpoint
//! (Unix domain socket / Windows named pipe). Lifecycle, cross-platform
//! transport, and the request protocol are implemented for U-006.

mod config;
mod dlog;
mod handler;
mod server;
mod writer;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;

use locus_core::ipc::paths::Paths;
use locus_core::store::Store;

use crate::config::{Config, LogLevel, DEFAULT_IDLE_TIMEOUT_SECS};
use crate::dlog::DaemonLog;

/// Local Locus daemon.
#[derive(Parser, Debug)]
#[command(name = "locusd")]
#[command(version = locus_core::VERSION)]
#[command(about = "Local Locus daemon that keeps storage and search warm", long_about = None)]
struct Cli {
    /// Run in the foreground (does not detach). Detached start is performed by
    /// the launching client.
    #[arg(long)]
    foreground: bool,

    /// Never self-exit on idle.
    #[arg(long)]
    no_idle_exit: bool,

    /// Idle timeout in seconds before the daemon exits when no requests arrive.
    #[arg(long, value_name = "SECONDS")]
    idle_timeout: Option<u64>,

    /// Log level: off, error, warn, info, debug.
    #[arg(long, value_name = "LEVEL")]
    log_level: Option<String>,

    /// Data directory to use (defaults to $LOCUS_HOME or ~/.locus).
    #[arg(long, value_name = "DIR")]
    data_dir: Option<PathBuf>,
}

fn main() -> Result<()> {
    locus_core::logging::init();
    let cli = Cli::parse();

    let config = Config {
        data_dir: cli.data_dir.clone(),
        idle_timeout: std::time::Duration::from_secs(
            cli.idle_timeout.unwrap_or(DEFAULT_IDLE_TIMEOUT_SECS),
        ),
        no_idle_exit: cli.no_idle_exit,
        log_level: cli
            .log_level
            .as_deref()
            .map(LogLevel::parse)
            .unwrap_or(LogLevel::Warn),
        foreground: cli.foreground,
    };

    run(config)
}

fn run(config: Config) -> Result<()> {
    let paths = match &config.data_dir {
        Some(dir) => Paths::from_data_dir(dir.clone()),
        None => Paths::resolve().context("failed to resolve Locus data directory")?,
    };
    paths
        .ensure_dirs()
        .context("failed to create data directory")?;

    let log = DaemonLog::open(&paths.log_file(), config.log_level);
    log.info(&format!(
        "starting locusd {} (foreground={}, idle_timeout={}s, no_idle_exit={})",
        locus_core::VERSION,
        config.foreground,
        config.idle_timeout.as_secs(),
        config.no_idle_exit,
    ));

    let store = Store::open_at(paths.db_file()).context("failed to open Locus database")?;

    // Validate FTS5 consistency on startup. Never auto-reindex or delete data;
    // recovery must be explicit (via the `reindex` command).
    match store.fts_out_of_sync() {
        Ok(true) => {
            log.warn("FTS5 index is inconsistent with canonical rows; run `reindex` to repair")
        }
        Ok(false) => {}
        Err(err) => log.warn(&format!("could not validate FTS5 consistency: {err}")),
    }

    let listener = match server::bind(&paths) {
        Ok(listener) => listener,
        Err(err) => {
            log.error(&format!("failed to bind IPC endpoint: {err}"));
            anyhow::bail!("failed to bind IPC endpoint: {err}");
        }
    };
    log.info(&format!(
        "listening on {} ({})",
        paths.endpoint().display(),
        paths.endpoint().transport(),
    ));

    write_pid_file(&paths)?;

    let (shared, writer_join) = server::Shared::new(store, paths.clone(), config, log);

    install_signal_handler(Arc::clone(&shared));

    server::serve(Arc::clone(&shared), listener);
    shared.log().info("locusd stopped");

    // Drain the writer thread so queued writes (access tracking, reindex,
    // capture) land before the process exits. The shutdown marker is queued
    // after every other op already in flight; the writer finishes them all and
    // then exits. We cannot rely on dropping `shared` to close the channel:
    // the signal-handler closure keeps an Arc alive until process exit.
    shared.writer().submit_async(writer::WriterOp::Shutdown);
    if let Some(join) = writer_join {
        let _ = join.join();
    }

    cleanup(&paths);
    Ok(())
}

fn install_signal_handler(shared: Arc<server::Shared>) {
    let result = ctrlc::set_handler(move || {
        shared
            .log()
            .info("received termination signal; shutting down");
        shared.request_shutdown();
    });
    if let Err(err) = result {
        // Not fatal: the daemon still responds to the `stop` command and idle
        // timeout. Only one handler can be installed per process.
        tracing::debug!(target: "locusd", "could not install signal handler: {err}");
    }
}

fn write_pid_file(paths: &Paths) -> Result<()> {
    let pid = std::process::id();
    let pid_file = paths.pid_file();
    std::fs::write(&pid_file, format!("{pid}\n")).context("failed to write PID file")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&pid_file) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(&pid_file, perms);
        }
    }
    Ok(())
}

fn cleanup(paths: &Paths) {
    let _ = std::fs::remove_file(paths.pid_file());
    if let Some(socket) = paths.endpoint().socket_file() {
        let _ = std::fs::remove_file(socket);
    }
}
