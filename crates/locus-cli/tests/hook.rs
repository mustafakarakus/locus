use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Once;

use locus_core::memory::{ListFilter, MemoryType, NewMemory};
use locus_core::store::Store;
use tempfile::TempDir;

static BUILD_DAEMON: Once = Once::new();

fn locus() -> Command {
    Command::new(env!("CARGO_BIN_EXE_locus"))
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate parent")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn ensure_locusd_built() -> PathBuf {
    let root = workspace_root();
    let bin = root.join("target").join("debug").join(if cfg!(windows) {
        "locusd.exe"
    } else {
        "locusd"
    });

    BUILD_DAEMON.call_once(|| {
        let status = Command::new("cargo")
            .args(["build", "-p", "locusd"])
            .current_dir(&root)
            .status()
            .expect("build locusd");
        assert!(status.success(), "building locusd failed");
    });

    assert!(bin.exists(), "expected locusd binary at {}", bin.display());
    bin
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git command");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_commit(repo: &Path, message: &str) -> String {
    git(repo, &["add", "."]);
    git(repo, &["commit", "-m", message]);
    git(repo, &["rev-parse", "--verify", "HEAD"])
}

fn init_repo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();

    git(repo, &["init"]);
    git(repo, &["config", "user.name", "Locus Test"]);
    git(repo, &["config", "user.email", "locus-test@example.com"]);

    fs::write(repo.join("README.md"), "hello\n").unwrap();
    let _ = git_commit(repo, "initial commit");

    tmp
}

#[test]
fn hook_install_works() {
    let repo = init_repo();

    let out = locus()
        .args(["hook", "install", "--path"])
        .arg(repo.path())
        .output()
        .expect("run hook install");
    assert!(out.status.success(), "hook install failed");

    let hook = repo.path().join(".git/hooks/post-commit");
    assert!(hook.is_file());
    let text = fs::read_to_string(hook).unwrap();
    assert!(text.contains("LOCUS:POST_COMMIT:START"));
    assert!(text.contains("run-post-commit"));
}

#[test]
fn hook_uninstall_works() {
    let repo = init_repo();

    let status = locus()
        .args(["hook", "install", "--path"])
        .arg(repo.path())
        .status()
        .unwrap();
    assert!(status.success());

    let status = locus()
        .args(["hook", "uninstall", "--path"])
        .arg(repo.path())
        .status()
        .unwrap();
    assert!(status.success());

    let hook = repo.path().join(".git/hooks/post-commit");
    if hook.exists() {
        let text = fs::read_to_string(hook).unwrap();
        assert!(!text.contains("LOCUS:POST_COMMIT:START"));
        assert!(!text.contains("LOCUS:POST_COMMIT:END"));
    }
}

#[test]
fn existing_hook_is_preserved_or_safely_wrapped() {
    let repo = init_repo();
    let hook = repo.path().join(".git/hooks/post-commit");

    let original = "#!/usr/bin/env sh\necho custom-hook\n";
    fs::write(&hook, original).unwrap();

    let status = locus()
        .args(["hook", "install", "--path"])
        .arg(repo.path())
        .status()
        .unwrap();
    assert!(status.success());

    let installed = fs::read_to_string(&hook).unwrap();
    assert!(installed.contains("echo custom-hook"));
    assert!(installed.contains("LOCUS:POST_COMMIT:START"));

    let status = locus()
        .args(["hook", "uninstall", "--path"])
        .arg(repo.path())
        .status()
        .unwrap();
    assert!(status.success());

    let removed = fs::read_to_string(&hook).unwrap();
    assert!(removed.contains("echo custom-hook"));
    assert!(!removed.contains("LOCUS:POST_COMMIT:START"));
}

#[test]
fn commit_metadata_creates_memory() {
    let repo = init_repo();
    let locus_home = TempDir::new().unwrap();
    let locusd_bin = ensure_locusd_built();

    fs::write(repo.path().join("src.txt"), "one\n").unwrap();
    let sha = git_commit(repo.path(), "add src file");

    let out = locus()
        .args(["hook", "run-post-commit", "--path"])
        .arg(repo.path())
        .env("LOCUS_HOME", locus_home.path())
        .env("LOCUSD_BIN", &locusd_bin)
        .output()
        .expect("run post-commit ingest");

    assert!(
        out.status.success(),
        "run-post-commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let store = Store::open_at(locus_home.path().join("locus.db")).unwrap();
    let list = store
        .list_memories(ListFilter {
            namespace: None,
            memory_type: None,
            limit: Some(200),
        })
        .unwrap();

    let hit = list
        .iter()
        .find(|m| m.content.contains(&sha) && m.source.as_deref() == Some("git:post-commit"))
        .expect("expected commit metadata memory");

    assert!(hit.content.contains("Changed files:"));
    assert!(hit.content.contains("src.txt"));
}

#[test]
fn large_diff_is_not_stored() {
    let repo = init_repo();
    let locus_home = TempDir::new().unwrap();
    let locusd_bin = ensure_locusd_built();

    let large = "X".repeat(300_000);
    let secretish = "DIFF_ONLY_TOKEN_SHOULD_NOT_STORE";
    fs::write(
        repo.path().join("big.txt"),
        format!("{large}\n{secretish}\n{large}\n"),
    )
    .unwrap();
    let _sha = git_commit(repo.path(), "add very large file");

    let status = locus()
        .args(["hook", "run-post-commit", "--path"])
        .arg(repo.path())
        .env("LOCUS_HOME", locus_home.path())
        .env("LOCUSD_BIN", &locusd_bin)
        .status()
        .unwrap();
    assert!(status.success());

    let store = Store::open_at(locus_home.path().join("locus.db")).unwrap();
    let list = store
        .list_memories(ListFilter {
            namespace: None,
            memory_type: None,
            limit: Some(200),
        })
        .unwrap();

    let hit = list
        .iter()
        .find(|m| m.source.as_deref() == Some("git:post-commit"))
        .expect("expected hook memory");

    assert!(hit.content.contains("big.txt"));
    assert!(
        !hit.content.contains(secretish),
        "diff payload should not be stored"
    );
}

#[test]
fn hook_failure_does_not_corrupt_git_commit() {
    let repo = init_repo();

    let status = locus()
        .args(["hook", "install", "--path"])
        .arg(repo.path())
        .status()
        .unwrap();
    assert!(status.success());

    fs::write(repo.path().join("later.txt"), "later\n").unwrap();

    let out = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["add", "."])
        .output()
        .unwrap();
    assert!(out.status.success());

    // Force hook ingestion failure; commit itself must still succeed.
    let out = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["commit", "-m", "commit with failing locus hook"])
        .env("LOCUSD_BIN", "/definitely/missing/locusd")
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "commit should succeed even if hook ingestion fails: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let last = git(repo.path(), &["log", "-1", "--pretty=%s"]);
    assert_eq!(last, "commit with failing locus hook");
}

fn seed_context_store(locus_home: &Path) {
    let store = Store::open_at(locus_home.join("locus.db")).unwrap();
    store
        .insert_memory(NewMemory {
            namespace: Some("project:demo".into()),
            memory_type: MemoryType::Decision,
            title: "Database".into(),
            content: "Use Postgres for storage".into(),
            entities: vec![],
            importance: 70,
            source: None,
        })
        .unwrap();
    store
        .insert_memory(NewMemory {
            namespace: Some("project:demo".into()),
            memory_type: MemoryType::Fact,
            title: "Other detail".into(),
            content: "CI runs on GitHub Actions".into(),
            entities: vec![],
            importance: 50,
            source: None,
        })
        .unwrap();
}

fn spawn_hook_context(args: &[&str], locus_home: &Path, payload: &str) -> std::process::Output {
    let mut child = locus()
        .args(["hook", "context"])
        .args(args)
        .env("LOCUS_HOME", locus_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn hook context");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write payload");
    child.wait_with_output().expect("wait hook context")
}

#[test]
fn hook_context_prompt_payload_returns_brief() {
    let locus_home = TempDir::new().unwrap();
    seed_context_store(locus_home.path());

    let payload = r#"{"session_id":"s1","cwd":"/repo/demo","hook_event_name":"UserPromptSubmit","prompt":"database"}"#;
    let out = spawn_hook_context(&[], locus_home.path(), payload);

    assert!(
        out.status.success(),
        "hook context failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Database"));
    assert!(stdout.contains("Use Postgres for storage"));
    assert!(stdout.contains("# Locus Memory Brief"));
}

#[test]
fn hook_context_session_start_derives_namespace_from_cwd() {
    let locus_home = TempDir::new().unwrap();
    seed_context_store(locus_home.path());

    let payload = r#"{"session_id":"s1","cwd":"/repo/demo","hook_event_name":"SessionStart"}"#;
    let out = spawn_hook_context(&[], locus_home.path(), payload);

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Database"),
        "session-start summary should include the scoped decision: {stdout}"
    );
}

#[test]
fn hook_context_unrelated_session_returns_no_relevant_memory() {
    let locus_home = TempDir::new().unwrap();
    seed_context_store(locus_home.path());

    let payload = r#"{"session_id":"s1","cwd":"/repo/other","hook_event_name":"SessionStart"}"#;
    let out = spawn_hook_context(&[], locus_home.path(), payload);

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim(), "NO_RELEVANT_MEMORY");
}

#[test]
fn hook_context_query_override_bypasses_payload() {
    let locus_home = TempDir::new().unwrap();
    seed_context_store(locus_home.path());

    let out = spawn_hook_context(
        &[
            "--query",
            "database",
            "--namespace",
            "project:demo",
            "--token-budget",
            "200",
        ],
        locus_home.path(),
        "not json at all",
    );

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Database"));
}

#[test]
fn hook_context_degrades_gracefully_on_invalid_payload() {
    let locus_home = TempDir::new().unwrap();

    let out = spawn_hook_context(&[], locus_home.path(), "definitely not json");

    assert!(
        out.status.success(),
        "hook must not block the host on failure"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim(), "NO_RELEVANT_MEMORY");
}
