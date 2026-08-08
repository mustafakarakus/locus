//! CLI integration tests for `locus graph` (U-016).

use std::fs;
use std::process::Command;

use tempfile::TempDir;

fn locus() -> Command {
    Command::new(env!("CARGO_BIN_EXE_locus"))
}

/// Runs a subcommand against an isolated data dir (HOME and LOCUS_HOME both
/// point at the temp dir so the real `~/.locus` is never touched).
fn run_in(dir: &TempDir, args: &[&str]) -> std::process::Output {
    locus()
        .args(args)
        .env("HOME", dir.path())
        .env("LOCUS_HOME", dir.path())
        .output()
        .expect("run locus")
}

#[test]
fn graph_snapshot_writes_self_contained_html() {
    let tmp = TempDir::new().unwrap();

    let seeded = run_in(
        &tmp,
        &["remember", "Adopt FTS5", "--entities", "sqlite search"],
    );
    assert!(
        seeded.status.success(),
        "seed failed: {}",
        String::from_utf8_lossy(&seeded.stderr)
    );

    let out = tmp.path().join("graph.html");
    let result = run_in(
        &tmp,
        &["graph", "--no-open", "--output", out.to_str().unwrap()],
    );
    assert!(
        result.status.success(),
        "graph failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(out.is_file(), "snapshot file must be written");

    let html = fs::read_to_string(&out).unwrap();
    assert!(html.contains("const GRAPH_DATA ="), "must embed graph data");
    assert!(html.contains("Adopt FTS5"), "must contain the memory title");
    assert!(html.contains("sqlite"), "must contain shared entity data");
    // Self-contained: no external resources.
    assert!(!html.contains("<script src="));
    assert!(!html.contains("<link "));
    assert!(!html.contains("<img "));
}

#[test]
fn graph_snapshot_of_empty_store_is_valid() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("graph.html");
    let result = run_in(
        &tmp,
        &["graph", "--no-open", "--output", out.to_str().unwrap()],
    );
    assert!(result.status.success());
    let html = fs::read_to_string(&out).unwrap();
    assert!(html.contains("\"nodes\":[]"));
    assert!(html.contains("const GRAPH_DATA ="));
}

#[test]
fn graph_help_lists_options() {
    let tmp = TempDir::new().unwrap();
    let output = run_in(&tmp, &["graph", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for flag in [
        "--namespace",
        "--expand",
        "--max-nodes",
        "--live",
        "--output",
    ] {
        assert!(stdout.contains(flag), "graph help missing {flag}");
    }
}
