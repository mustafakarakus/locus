//! Packaging and release integration tests (U-014).

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

fn locus() -> Command {
    Command::new(env!("CARGO_BIN_EXE_locus"))
}

/// Repository root: <manifest>/../..
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

/// Directory containing the freshly built debug binaries.
fn built_bin_dir() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_locus"))
        .parent()
        .expect("bin dir")
        .to_path_buf()
}

fn script_path(name: &str) -> PathBuf {
    repo_root().join("scripts").join(name)
}

// ---------------------------------------------------------------------------
// Binary starts
// ---------------------------------------------------------------------------

#[test]
fn binary_starts_and_reports_version() {
    let out = locus()
        .arg("--version")
        .output()
        .expect("run locus --version");
    assert!(out.status.success(), "locus --version failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("0.1.0"),
        "unexpected version output: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// Shell completions generate
// ---------------------------------------------------------------------------

#[test]
fn completions_generate_for_each_shell() {
    let cases = [
        ("bash", "_locus"),
        ("zsh", "#compdef locus"),
        ("fish", "complete"),
    ];
    for (shell, marker) in cases {
        let out = locus()
            .args(["completions", shell])
            .output()
            .expect("run completions");
        assert!(
            out.status.success(),
            "completions {shell} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(marker),
            "completions {shell} missing {marker:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// `locus doctor` passes on a clean install
// ---------------------------------------------------------------------------

#[test]
fn doctor_passes_on_clean_install() {
    let tmp = TempDir::new().expect("temp dir");
    let out = locus()
        .env("LOCUS_HOME", tmp.path())
        .arg("doctor")
        .output()
        .expect("run doctor");
    assert!(
        out.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("All checks passed"),
        "doctor should pass on clean install, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// Install script works in a temporary directory
// ---------------------------------------------------------------------------

#[test]
fn install_script_installs_to_temp_prefix() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");

    let out = Command::new("sh")
        .arg(script_path("install.sh"))
        .arg("--from")
        .arg(built_bin_dir())
        .arg("--bin-dir")
        .arg(&bin_dir)
        .arg("--no-init")
        .output()
        .expect("run install.sh");
    assert!(
        out.status.success(),
        "install.sh failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    for bin in ["locus", "locusd", "locus-mcp", "locus-viz"] {
        let path = bin_dir.join(bin);
        assert!(path.is_file(), "expected {path:?} to be installed");
    }

    let version = Command::new(bin_dir.join("locus"))
        .arg("--version")
        .output()
        .expect("run installed locus --version");
    assert!(version.status.success());
}

// ---------------------------------------------------------------------------
// Uninstall script works
// ---------------------------------------------------------------------------

#[test]
fn uninstall_script_removes_installed_binaries() {
    let tmp = TempDir::new().expect("temp dir");
    let bin_dir = tmp.path().join("bin");

    let install = Command::new("sh")
        .arg(script_path("install.sh"))
        .arg("--from")
        .arg(built_bin_dir())
        .arg("--bin-dir")
        .arg(&bin_dir)
        .arg("--no-init")
        .output()
        .expect("run install.sh");
    assert!(install.status.success());

    let out = Command::new("sh")
        .arg(script_path("uninstall.sh"))
        .arg("--bin-dir")
        .arg(&bin_dir)
        .output()
        .expect("run uninstall.sh");
    assert!(
        out.status.success(),
        "uninstall.sh failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    for bin in ["locus", "locusd", "locus-mcp", "locus-viz"] {
        assert!(!bin_dir.join(bin).exists(), "expected {bin} to be removed");
    }
}
