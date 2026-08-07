//! CLI integration tests for `locus init` (U-008).

use std::fs;
use std::process::Command;

use tempfile::TempDir;

fn locus() -> Command {
    Command::new(env!("CARGO_BIN_EXE_locus"))
}

#[test]
fn yes_flag_works_non_interactively() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    let output = locus()
        .args(["init", "--yes", "--path"])
        .arg(root)
        .output()
        .expect("run locus init --yes");

    assert!(
        output.status.success(),
        "init --yes failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Initialized") || stdout.contains("wrote"),
        "unexpected stdout: {stdout}"
    );

    assert!(root.join("CLAUDE.md").is_file());
    assert!(root.join(".cursorrules").is_file());
    assert!(root.join(".clinerules").is_file());
    assert!(root.join(".mcp.json").is_file());
    assert!(root.join(".cursor/mcp.json").is_file());

    let claude = fs::read_to_string(root.join("CLAUDE.md")).unwrap();
    assert!(claude.contains("LOCUS:MEMORY_PROTOCOL:START"));
    assert!(claude.contains("memory_search"));
}

#[test]
fn yes_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    for _ in 0..2 {
        let status = locus()
            .args(["init", "--yes", "--path"])
            .arg(root)
            .status()
            .unwrap();
        assert!(status.success());
    }

    let text = fs::read_to_string(root.join("CLAUDE.md")).unwrap();
    assert_eq!(text.matches("LOCUS:MEMORY_PROTOCOL:START").count(), 1);
}

#[test]
fn dry_run_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    let output = locus()
        .args(["init", "--dry-run", "--path"])
        .arg(root)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!root.join("CLAUDE.md").exists());
    assert!(!root.join(".mcp.json").exists());
}

#[test]
fn init_help_lists_yes() {
    let output = locus().args(["init", "--help"]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--yes"));
    assert!(stdout.contains("--path"));
    assert!(stdout.contains("--dry-run"));
}

#[test]
fn refuses_without_yes_when_non_interactive() {
    let tmp = TempDir::new().unwrap();
    // No TTY and no --yes → should refuse rather than hang or silently write.
    let output = locus()
        .args(["init", "--path"])
        .arg(tmp.path())
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "expected failure without --yes on non-TTY"
    );
    assert!(!tmp.path().join("CLAUDE.md").exists());
}
