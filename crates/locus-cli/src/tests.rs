use std::process::Command;
use tempfile::TempDir;

fn setup_test_db() -> TempDir {
    TempDir::new().expect("failed to create temp dir")
}

#[test]
fn remember_command_works() {
    let _temp_dir = setup_test_db();
    let status = Command::new("cargo")
        .args(["run", "--"])
        .arg("remember")
        .arg("test memory")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("failed to run remember command");

    assert!(status.success(), "remember command failed");
}

#[test]
fn help_text_is_available() {
    let output = Command::new("cargo")
        .args(["run", "--", "--help"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run help command");

    assert!(output.status.success(), "help command failed");
    let stdout = String::from_utf8(output.stdout).expect("invalid UTF-8");
    assert!(
        stdout.contains("Local-first"),
        "help text missing description"
    );
    assert!(
        stdout.contains("Commands:"),
        "help text missing commands section"
    );
}

#[test]
fn version_flag_works() {
    let output = Command::new("cargo")
        .args(["run", "--", "--version"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run version command");

    assert!(output.status.success(), "version command failed");
    let stdout = String::from_utf8(output.stdout).expect("invalid UTF-8");
    assert!(
        stdout.contains("locus"),
        "version output should contain 'locus'"
    );
}

#[test]
fn remember_help_shows_all_options() {
    let output = Command::new("cargo")
        .args(["run", "--", "remember", "--help"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run remember help");

    assert!(output.status.success(), "remember help failed");
    let stdout = String::from_utf8(output.stdout).expect("invalid UTF-8");
    assert!(
        stdout.contains("--type"),
        "remember help missing --type option"
    );
    assert!(
        stdout.contains("--namespace"),
        "remember help missing --namespace option"
    );
    assert!(
        stdout.contains("--importance"),
        "remember help missing --importance option"
    );
    assert!(
        stdout.contains("--json"),
        "remember help missing --json option"
    );
}

#[test]
fn search_help_shows_all_options() {
    let output = Command::new("cargo")
        .args(["run", "--", "search", "--help"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run search help");

    assert!(output.status.success(), "search help failed");
    let stdout = String::from_utf8(output.stdout).expect("invalid UTF-8");
    assert!(
        stdout.contains("--namespace"),
        "search help missing --namespace option"
    );
    assert!(
        stdout.contains("--type"),
        "search help missing --type option"
    );
    assert!(
        stdout.contains("--limit"),
        "search help missing --limit option"
    );
    assert!(
        stdout.contains("--json"),
        "search help missing --json option"
    );
}

#[test]
fn all_commands_are_listed_in_help() {
    let output = Command::new("cargo")
        .args(["run", "--", "--help"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run help");

    assert!(output.status.success(), "help command failed");
    let stdout = String::from_utf8(output.stdout).expect("invalid UTF-8");
    assert!(
        stdout.contains("remember"),
        "help missing 'remember' command"
    );
    assert!(stdout.contains("search"), "help missing 'search' command");
    assert!(stdout.contains("context"), "help missing 'context' command");
    assert!(stdout.contains("forget"), "help missing 'forget' command");
    assert!(stdout.contains("status"), "help missing 'status' command");
    assert!(stdout.contains("doctor"), "help missing 'doctor' command");
    assert!(stdout.contains("reindex"), "help missing 'reindex' command");
    assert!(stdout.contains("daemon"), "help missing 'daemon' command");
    assert!(stdout.contains("mcp"), "help missing 'mcp' command");
}
