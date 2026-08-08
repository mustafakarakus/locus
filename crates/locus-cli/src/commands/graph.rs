//! `locus graph` — render the memory graph as an offline HTML snapshot or a
//! live loopback view (U-016).
//!
//! Snapshot mode reads the graph on a dedicated read-only connection and writes
//! a single self-contained HTML file (all CSS/JS inlined, no network calls).
//! Live mode spawns the `locus-viz` process, which serves the same page over
//! loopback HTTP plus an SSE stream driven by daemon events.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use locus_core::graph::{GraphRequest, DEFAULT_GRAPH_MAX_NODES};
use locus_core::ipc::paths::Paths;
use locus_core::ipc::DaemonClient;
use locus_core::store::Store;
use locus_core::viz;

/// Environment variable that overrides the discovered `locus-viz` binary.
const LOCUS_VIZ_BIN_ENV: &str = "LOCUS_VIZ_BIN";

/// Render the memory graph
#[derive(Parser, Debug)]
pub struct GraphCmd {
    /// Filter the graph to a single namespace
    #[arg(short, long)]
    pub namespace: Option<String>,

    /// Focus a single memory and its immediate neighbors (shared entities)
    #[arg(long)]
    pub expand: Option<String>,

    /// Maximum number of nodes to include
    #[arg(long, default_value_t = DEFAULT_GRAPH_MAX_NODES)]
    pub max_nodes: usize,

    /// Output file for the snapshot HTML (default: <data-dir>/graph.html)
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Do not try to open the snapshot in a browser
    #[arg(long)]
    pub no_open: bool,

    /// Stream live updates from the daemon through a loopback server
    #[arg(long)]
    pub live: bool,
}

impl GraphCmd {
    pub fn run(self) -> Result<()> {
        if self.live {
            self.run_live()
        } else {
            self.run_snapshot()
        }
    }

    fn run_snapshot(&self) -> Result<()> {
        let store = Store::open_default()?;
        let data = store.graph(GraphRequest {
            namespace: self.namespace.clone(),
            expand: self.expand.clone(),
            max_nodes: self.max_nodes,
        })?;

        let html = viz::snapshot_html(&data)?;

        let path = match &self.output {
            Some(path) => path.clone(),
            None => Paths::resolve()?.graph_file(),
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, html).with_context(|| format!("failed to write {}", path.display()))?;

        if self.no_open {
            println!("Wrote graph to {}", path.display());
        } else {
            println!("Graph written to {}", path.display());
            open_in_browser(&path.to_string_lossy());
        }
        Ok(())
    }

    fn run_live(&self) -> Result<()> {
        let paths = Paths::resolve()?;

        // Live updates come from daemon events; make sure it is running.
        let client = DaemonClient::new(paths.endpoint().clone());
        if !client.is_running() {
            let daemon_bin = crate::commands::daemon::locate_daemon_binary()?;
            client
                .connect_or_spawn(&daemon_bin, paths.data_dir())
                .map_err(|err| anyhow!("failed to start daemon: {err}"))?;
        }

        let viz_bin = locate_viz_binary()?;
        let mut child = Command::new(&viz_bin)
            .arg("--data-dir")
            .arg(paths.data_dir())
            .arg("--max-nodes")
            .arg(self.max_nodes.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("failed to start {}", viz_bin.display()))?;

        // locus-viz prints its URL as the first stdout line.
        use std::io::{BufRead, BufReader};
        let mut stdout = BufReader::new(child.stdout.take().expect("viz stdout"));
        let mut url = String::new();
        let read = stdout
            .read_line(&mut url)
            .context("locus-viz printed no URL")?;
        if read == 0 {
            return Err(anyhow!("locus-viz exited without printing a URL"));
        }
        let url = url.trim().to_string();

        println!("Live graph: {url}");
        println!("Press Ctrl-C to stop.");
        open_in_browser(&url);

        // Keep the CLI alive as the parent so Ctrl-C also terminates the
        // viewer; exit when the viewer goes away on its own.
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
                Err(_) => return Ok(()),
            }
        }
    }
}

/// Best-effort browser open (ignored on failure so headless use still works).
fn open_in_browser(target: &str) {
    let _ = open_command(target);
}

fn open_command(target: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(target).status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open").arg(target).status();
    #[cfg(windows)]
    let status = Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(target)
        .status();
    let _ = status;
    Ok(())
}

/// Locates the `locus-viz` executable.
///
/// Resolution order:
/// 1. The `LOCUS_VIZ_BIN` environment variable, if set.
/// 2. A `locus-viz` binary sitting next to the current `locus` executable.
/// 3. Bare `locus-viz`, relying on `PATH`.
fn locate_viz_binary() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var(LOCUS_VIZ_BIN_ENV) {
        if !custom.trim().is_empty() {
            return Ok(PathBuf::from(custom));
        }
    }

    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            let sibling = dir.join(viz_file_name());
            if sibling.exists() {
                return Ok(sibling);
            }
        }
    }

    Ok(PathBuf::from("locus-viz"))
}

#[cfg(windows)]
fn viz_file_name() -> &'static str {
    "locus-viz.exe"
}

#[cfg(not(windows))]
fn viz_file_name() -> &'static str {
    "locus-viz"
}
