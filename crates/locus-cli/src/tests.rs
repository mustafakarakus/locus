use std::process::Command;
use tempfile::TempDir;

/// Runs a locus subcommand with a fully isolated LOCUS_HOME, so tests never
/// touch the developer's real `~/.locus` database.
fn run_locus(home: &TempDir, args: &[&str]) -> std::process::Output {
    Command::new("cargo")
        .args(["run", "--quiet", "--bin", "locus", "--"])
        .args(args)
        .env(locus_core::ipc::paths::HOME_ENV, home.path())
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run locus")
}

fn setup_test_db() -> TempDir {
    TempDir::new().expect("failed to create temp dir")
}

#[test]
fn remember_command_works() {
    let home = setup_test_db();
    let output = run_locus(&home, &["remember", "test memory"]);
    assert!(
        output.status.success(),
        "remember command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("✓ Remembered"));
}

#[test]
fn remember_json_is_valid() {
    let home = setup_test_db();
    let output = run_locus(
        &home,
        &[
            "remember",
            "--json",
            "--title",
            "title with \"quotes\"",
            "body",
        ],
    );
    assert!(
        output.status.success(),
        "remember failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("remember --json must emit valid JSON");
    assert_eq!(parsed["status"], "ok");
    assert!(parsed["id"].is_string());
}

#[test]
fn forget_all_requires_confirmation_and_wipes_every_memory() {
    let home = setup_test_db();
    assert!(run_locus(&home, &["remember", "first wipe target"])
        .status
        .success());
    assert!(run_locus(&home, &["remember", "second wipe target"])
        .status
        .success());

    let refused = run_locus(&home, &["forget", "--all"]);
    assert!(!refused.status.success(), "wipe must require --yes");

    let output = run_locus(&home, &["forget", "--all", "--yes", "--json"]);
    assert!(
        output.status.success(),
        "wipe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["deleted"], 2);

    let status = run_locus(&home, &["status", "--json"]);
    let parsed: serde_json::Value = serde_json::from_slice(&status.stdout).expect("valid status");
    assert_eq!(parsed["memory_count"], 0);
    assert_eq!(parsed["fts_row_count"], 0);

    assert!(
        run_locus(&home, &["remember", "fresh start works"])
            .status
            .success(),
        "store must remain usable after wipe"
    );
}

#[test]
fn reindex_command_rebuilds_index() {
    let home = setup_test_db();
    let remember = run_locus(&home, &["remember", "reindexable content"]);
    assert!(remember.status.success());

    let output = run_locus(&home, &["reindex", "--json"]);
    assert!(
        output.status.success(),
        "reindex failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("reindex --json must emit valid JSON");
    assert_eq!(parsed["status"], "ok");
    assert!(parsed["reindexed"].as_u64().unwrap_or(0) >= 1);

    let search = run_locus(&home, &["search", "--json", "reindexable"]);
    assert!(search.status.success());
    let stdout = String::from_utf8_lossy(&search.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("search --json must emit valid JSON");
    assert_eq!(parsed["status"], "ok");
    assert!(
        parsed["count"].as_u64().unwrap_or(0) >= 1,
        "memory must be searchable after reindex"
    );
}

#[test]
fn doctor_reports_ok_and_valid_json() {
    let home = setup_test_db();
    let output = run_locus(&home, &["doctor", "--json"]);
    assert!(
        output.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("doctor --json must emit valid JSON");
    assert_eq!(parsed["status"], "ok");
    assert!(parsed["issues"].is_array());
}

#[test]
fn status_reports_real_counts() {
    let home = setup_test_db();
    run_locus(&home, &["remember", "status check memory"]);

    let output = run_locus(&home, &["status", "--json"]);
    assert!(
        output.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("status --json must emit valid JSON");
    assert!(parsed["memory_count"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(parsed["database"], "ok");
}

#[test]
fn help_text_is_available() {
    let home = setup_test_db();
    let output = run_locus(&home, &["--help"]);
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
    let home = setup_test_db();
    let output = run_locus(&home, &["--version"]);
    assert!(output.status.success(), "version command failed");
    let stdout = String::from_utf8(output.stdout).expect("invalid UTF-8");
    assert!(
        stdout.contains("locus"),
        "version output should contain 'locus'"
    );
}

#[test]
fn remember_help_shows_all_options() {
    let home = setup_test_db();
    let output = run_locus(&home, &["remember", "--help"]);
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
    let home = setup_test_db();
    let output = run_locus(&home, &["search", "--help"]);
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
    let home = setup_test_db();
    let output = run_locus(&home, &["--help"]);
    assert!(output.status.success(), "help command failed");
    let stdout = String::from_utf8(output.stdout).expect("invalid UTF-8");
    assert!(stdout.contains("init"), "help missing 'init' command");
    assert!(
        stdout.contains("remember"),
        "help missing 'remember' command"
    );
    assert!(stdout.contains("search"), "help missing 'search' command");
    assert!(stdout.contains("context"), "help missing 'context' command");
    assert!(stdout.contains("forget"), "help missing 'forget' command");
    assert!(stdout.contains("hook"), "help missing 'hook' command");
    assert!(stdout.contains("status"), "help missing 'status' command");
    assert!(stdout.contains("doctor"), "help missing 'doctor' command");
    assert!(stdout.contains("reindex"), "help missing 'reindex' command");
    assert!(stdout.contains("daemon"), "help missing 'daemon' command");
    assert!(stdout.contains("mcp"), "help missing 'mcp' command");
}
