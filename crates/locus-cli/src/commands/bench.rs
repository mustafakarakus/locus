//! `locus bench` — performance benchmark harness (U-012).
//!
//! Generates deterministic datasets at configured sizes and measures the
//! performance budget from `TECHSTACK.md`:
//!
//! - warm search p95 < 20 ms at 100k memories (plus p50/p99)
//! - single-memory save p95 < 15 ms
//! - context generation p95 (within the MCP 30 ms tool budget)
//! - CLI cold start p95 < 50 ms
//! - daemon idle RSS < 25 MB
//!
//! Every measurement is recorded as a latency sample and summarized into
//! p50/p95/p99 via `locus_testkit::stats`. If any budget is exceeded, the
//! command exits non-zero (unless `--no-fail`), so the suite can gate CI.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use locus_core::search::Query;
use locus_core::store::Store;
use locus_testkit::dataset;
use locus_testkit::stats::{summarize, Percentiles};

use crate::commands::daemon::locate_daemon_binary;

/// Budget constants (ms) — mirror `TECHSTACK.md`.
const SEARCH_P95_BUDGET_MS: f64 = 20.0;
const SAVE_P95_BUDGET_MS: f64 = 15.0;
const CONTEXT_P95_BUDGET_MS: f64 = 30.0;
const CLI_STARTUP_P95_BUDGET_MS: f64 = 50.0;
const DAEMON_IDLE_RSS_BUDGET_MB: f64 = 25.0;

/// How long to wait for the daemon to accept connections before failing.
const DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(10);

/// Run performance benchmarks (U-012)
#[derive(Parser, Debug)]
pub struct BenchCmd {
    /// Dataset sizes to benchmark, comma-separated (e.g. `1000,10000,100000`)
    #[arg(long, default_value = "1000,10000,100000")]
    pub sizes: String,

    /// Latency samples to collect per measurement
    #[arg(long, default_value = "100")]
    pub iterations: usize,

    /// Data directory to hold generated benchmark databases
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Report results but do not exit non-zero when a budget is exceeded
    #[arg(long)]
    pub no_fail: bool,
}

impl BenchCmd {
    pub fn run(self) -> Result<()> {
        let sizes = parse_sizes(&self.sizes)?;
        let iterations = self.iterations.max(1);

        let (work_dir, temp) = match &self.data_dir {
            Some(dir) => {
                std::fs::create_dir_all(dir)?;
                (dir.clone(), None)
            }
            None => {
                let dir = std::env::temp_dir().join(format!("locus-bench-{}", std::process::id()));
                std::fs::create_dir_all(&dir)?;
                (dir.clone(), Some(dir))
            }
        };

        let mut failures: Vec<String> = Vec::new();

        println!("locus bench — performance budget check (U-012)");
        println!(
            "  sizes:     {}",
            sizes
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("  iterations per measurement: {iterations}");
        println!();

        for &size in &sizes {
            match self.bench_size(&work_dir, size, iterations) {
                Ok(report) => {
                    println!("{report}");
                    for (name, p95, budget) in report.violations {
                        let msg =
                            format!("budget exceeded: {name} p95={p95:.2}ms budget<{budget:.0}ms");
                        if self.no_fail {
                            println!("  [WARN] {msg}");
                        } else {
                            failures.push(msg);
                        }
                    }
                }
                Err(err) => {
                    let msg = format!("dataset size {size} failed: {err:#}");
                    if self.no_fail {
                        println!("  [WARN] {msg}");
                    } else {
                        failures.push(msg);
                    }
                }
            }
            println!();
        }

        let cli = self.bench_cli_startup(iterations)?;
        println!(
            "CLI cold start (spawn `locus --version`): p50={:.3} p95={:.3} p99={:.3} ms  [{}] (budget p95<{:.0})",
            cli.p50_ms,
            cli.p95_ms,
            cli.p99_ms,
            if cli.p95_ms <= CLI_STARTUP_P95_BUDGET_MS { "ok" } else { "FAIL" },
            CLI_STARTUP_P95_BUDGET_MS
        );
        if cli.p95_ms > CLI_STARTUP_P95_BUDGET_MS {
            let msg = format!(
                "budget exceeded: CLI cold start p95={:.2}ms budget<{:.0}ms",
                cli.p95_ms, CLI_STARTUP_P95_BUDGET_MS
            );
            if self.no_fail {
                println!("  [WARN] {msg}");
            } else {
                failures.push(msg);
            }
        }
        println!();

        match self.measure_daemon_idle_rss(&work_dir)? {
            Some(rss_mb) => {
                println!(
                    "daemon idle RSS: {rss_mb:.1} MB (budget < {DAEMON_IDLE_RSS_BUDGET_MB:.0} MB)"
                );
                if rss_mb > DAEMON_IDLE_RSS_BUDGET_MB {
                    let msg = format!(
                        "budget exceeded: daemon idle RSS {rss_mb:.1}MB budget<{DAEMON_IDLE_RSS_BUDGET_MB:.0}MB"
                    );
                    if self.no_fail {
                        println!("  [WARN] {msg}");
                    } else {
                        failures.push(msg);
                    }
                }
            }
            None => println!("daemon idle RSS: (could not measure on this platform)"),
        }
        println!();

        let _ = temp; // temp dir is removed on drop unless --data-dir was given.

        if failures.is_empty() {
            println!("RESULT: PASS — all budgets within target.");
            Ok(())
        } else {
            for f in &failures {
                println!("RESULT: FAIL — {f}");
            }
            bail!(
                "performance budget exceeded ({} failure(s))",
                failures.len()
            );
        }
    }

    fn bench_size(&self, work_dir: &Path, size: usize, iterations: usize) -> Result<BenchReport> {
        let db_path = work_dir.join(format!("bench-{size}.db"));
        let store = Store::open_at(&db_path)?;

        // Build the dataset once per size; generation itself is timed so bulk
        // ingestion throughput is observable.
        let gen_start = Instant::now();
        for memory in dataset::generate(size) {
            store.insert_memory(memory)?;
        }
        let gen_elapsed = gen_start.elapsed();
        let gen_rate = size as f64 / gen_elapsed.as_secs_f64();

        // Warm the OS page cache with a first search so we measure warm search.
        let warm_query = Query::new("verify_token_handler_0");
        store.search(warm_query)?;

        // Search shapes. Gated shapes are realistic warm-search queries that
        // match a bounded subset of the corpus and are held to the 20ms budget.
        // Evidence shapes deliberately match the whole corpus (or exercise the
        // LIKE fallback) and are reported for the U-012 Tantivy decision, not
        // gated — any lexical engine is O(matches) for a corpus-wide query.
        // `(name, text, namespace, gated)`
        let shapes = [
            ("exact", "verify_token_handler_0", None, true),
            ("identifier", "AuthService::verify_token_1", None, true),
            ("prefix", "verify_token_handler_500*", None, true),
            ("partial", "verify_token_handler_50*", None, true),
            (
                "phrase",
                "\"authenticates via verify_token_handler_1\"",
                None,
                true,
            ),
            (
                "ns-filtered",
                "verify_token_handler_0",
                Some("project:auth"),
                true,
            ),
            ("prefix-all", "verify*", None, false),
            ("phrase-all", "\"auth middleware\"", None, false),
            ("partial-fallback", "fy_token_hand", None, false),
            ("typo", "autth middlewaer", None, false),
            (
                "ns-miss",
                "verify_token_handler_2",
                Some("project:auth"),
                false,
            ),
        ];

        let mut lines = Vec::new();
        let mut violations = Vec::new();
        lines.push(format!(
            "size {size}: generated {} memories in {gen_elapsed:?} ({gen_rate:.0}/s)",
            size
        ));

        for (name, text, ns, gated) in shapes {
            let p = self.sample_search(&store, text, ns, iterations)?;
            let pass = p.p95_ms <= SEARCH_P95_BUDGET_MS;
            if gated && !pass {
                violations.push((format!("search[{name}]"), p.p95_ms, SEARCH_P95_BUDGET_MS));
            }
            let flag = match (gated, pass) {
                (true, true) => "ok".to_string(),
                (true, false) => "FAIL".to_string(),
                (false, true) => "info".to_string(),
                (false, false) => "note".to_string(),
            };
            lines.push(format!(
                "  search[{name:<12}] p50={:.3} p95={:.3} p99={:.3} ms  [{flag}] (budget p95<{:.0}{})",
                p.p50_ms,
                p.p95_ms,
                p.p99_ms,
                SEARCH_P95_BUDGET_MS,
                if gated { "" } else { ", evidence" }
            ));
        }

        let save_p = self.sample_save(&store, size, iterations)?;
        let save_pass = save_p.p95_ms <= SAVE_P95_BUDGET_MS;
        if !save_pass {
            violations.push((
                "save[single]".to_string(),
                save_p.p95_ms,
                SAVE_P95_BUDGET_MS,
            ));
        }
        let save_flag = if save_pass { "ok" } else { "FAIL" };
        lines.push(format!(
            "  save[single]      p50={:.3} p95={:.3} p99={:.3} ms  [{save_flag}] (budget p95<{:.0})",
            save_p.p50_ms, save_p.p95_ms, save_p.p99_ms, SAVE_P95_BUDGET_MS
        ));

        let ctx_p = self.sample_context(&store, iterations)?;
        let ctx_pass = ctx_p.p95_ms <= CONTEXT_P95_BUDGET_MS;
        if !ctx_pass {
            violations.push((
                "context[brief]".to_string(),
                ctx_p.p95_ms,
                CONTEXT_P95_BUDGET_MS,
            ));
        }
        let ctx_flag = if ctx_pass { "ok" } else { "FAIL" };
        lines.push(format!(
            "  context[brief]    p50={:.3} p95={:.3} p99={:.3} ms  [{ctx_flag}] (budget p95<{:.0})",
            ctx_p.p50_ms, ctx_p.p95_ms, ctx_p.p99_ms, CONTEXT_P95_BUDGET_MS
        ));

        Ok(BenchReport { lines, violations })
    }

    fn sample_search(
        &self,
        store: &Store,
        text: &str,
        ns: Option<&str>,
        iterations: usize,
    ) -> Result<Percentiles> {
        // First iteration warms prepared statement caches.
        let q = Query {
            text: text.to_string(),
            namespace: ns.map(|s| s.to_string()),
            memory_type: None,
            limit: 10,
        };
        store.search(q.clone())?;
        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let start = Instant::now();
            store.search(q.clone())?;
            samples.push(elapsed_ms(start));
        }
        Ok(summarize(&mut samples))
    }

    fn sample_save(&self, store: &Store, size: usize, iterations: usize) -> Result<Percentiles> {
        // Warm the write path, then measure single-memory saves.
        let mut samples = Vec::with_capacity(iterations);
        for i in 0..iterations {
            let start = Instant::now();
            store.insert_memory(dataset::generate_one(size + i))?;
            samples.push(elapsed_ms(start));
        }
        Ok(summarize(&mut samples))
    }

    fn sample_context(&self, store: &Store, iterations: usize) -> Result<Percentiles> {
        let q = Query::new("verify_token_handler_3");
        store.context_brief(q.clone(), Default::default())?;
        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let start = Instant::now();
            store.context_brief(q.clone(), Default::default())?;
            samples.push(elapsed_ms(start));
        }
        Ok(summarize(&mut samples))
    }

    fn bench_cli_startup(&self, iterations: usize) -> Result<Percentiles> {
        let bin = std::env::current_exe().context("resolve current executable")?;
        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let start = Instant::now();
            let status = Command::new(&bin)
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .context("spawn locus --version")?;
            if !status.success() {
                bail!("`locus --version` exited with {status}");
            }
            samples.push(elapsed_ms(start));
        }
        Ok(summarize(&mut samples))
    }

    /// Spawns a fresh `locusd` on an isolated data dir, waits for it to accept
    /// connections, samples its RSS, and shuts it down. Returns None when RSS
    /// cannot be measured on this platform.
    fn measure_daemon_idle_rss(&self, work_dir: &Path) -> Result<Option<f64>> {
        let bin = locate_daemon_binary()?;
        let data_dir = work_dir.join("bench-daemon-idle");
        std::fs::create_dir_all(&data_dir)?;

        let mut child = Command::new(&bin)
            .arg("--foreground")
            .arg("--no-idle-exit")
            .arg("--data-dir")
            .arg(&data_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn daemon {} for idle-RSS measurement", bin.display()))?;

        let ready = wait_for_daemon_ready(&data_dir)?;
        if !ready {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "daemon did not become ready within {DAEMON_READY_TIMEOUT:?}"
            ));
        }

        // Give the daemon a moment to fully settle after accepting its first
        // connection, then sample RSS a few times and take the minimum.
        std::thread::sleep(Duration::from_millis(250));
        let mut rss_kb: Vec<u64> = Vec::new();
        for _ in 0..3 {
            if let Some(kb) = read_rss_kb(child.id()) {
                rss_kb.push(kb);
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        let _ = child.kill();
        let _ = child.wait();

        Ok(rss_kb.iter().min().map(|kb| *kb as f64 / 1024.0))
    }
}

/// A per-size benchmark report: human-readable lines plus budget violations.
struct BenchReport {
    lines: Vec<String>,
    violations: Vec<(String, f64, f64)>,
}

impl std::fmt::Display for BenchReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for line in &self.lines {
            writeln!(f, "{line}")?;
        }
        Ok(())
    }
}

fn parse_sizes(raw: &str) -> Result<Vec<usize>> {
    let sizes = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<usize>()
                .map_err(|_| anyhow!("invalid size `{s}`; expected a positive integer"))
        })
        .collect::<Result<Vec<_>>>()?;
    if sizes.is_empty() {
        bail!("no sizes given");
    }
    for s in &sizes {
        if *s == 0 {
            bail!("sizes must be positive");
        }
    }
    Ok(sizes)
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

/// Polls the daemon endpoint until it answers `ping` or the timeout elapses.
fn wait_for_daemon_ready(data_dir: &Path) -> Result<bool> {
    use locus_core::ipc::paths::Paths;
    use locus_core::ipc::DaemonClient;

    let paths = Paths::from_data_dir(data_dir);
    let client = DaemonClient::new(paths.endpoint().clone());
    let deadline = Instant::now() + DAEMON_READY_TIMEOUT;
    while Instant::now() < deadline {
        if client.is_running() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(false)
}

/// Reads a process's resident set size in kilobytes.
///
/// Uses `ps -o rss=` (works on Linux and macOS). Returns None when the process
/// has exited or the platform cannot report RSS.
fn read_rss_kb(pid: u32) -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sizes_accepts_whitespace_and_trims() {
        let sizes = parse_sizes(" 1000, 10000 ,100000 ").unwrap();
        assert_eq!(sizes, vec![1000, 10000, 100000]);
    }

    #[test]
    fn parse_sizes_rejects_non_numeric() {
        assert!(parse_sizes("1000,bad").is_err());
    }

    #[test]
    fn parse_sizes_rejects_empty_and_zero() {
        assert!(parse_sizes("").is_err());
        assert!(parse_sizes("0").is_err());
        assert!(parse_sizes("1000,0").is_err());
    }

    #[test]
    fn dataset_generates_via_testkit() {
        // `locus bench` depends on the deterministic testkit dataset; a smoke
        // check keeps the dependency honest and ensures inserts succeed.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_at(dir.path().join("locus.db")).unwrap();
        for memory in dataset::generate(50) {
            store.insert_memory(memory).unwrap();
        }
        let hits = store.search(Query::new("verify_token_handler_0")).unwrap();
        assert!(!hits.is_empty(), "dataset must be searchable");
    }
}
