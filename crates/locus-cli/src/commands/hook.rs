//! `locus hook` — Git hook ingestion (U-009) and lifecycle context injection (U-015).

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use locus_core::context::NO_RELEVANT_MEMORY;
use locus_core::hooks::{adapter_for, inject_context, DefaultQueryStrategy, InjectTrigger};
use locus_core::ipc::paths::Paths;
use locus_core::ipc::protocol::{command, RememberRequest, RememberResponse, Request};
use locus_core::ipc::DaemonClient;
use locus_core::store::Store;

const START_MARKER: &str = "# LOCUS:POST_COMMIT:START";
const END_MARKER: &str = "# LOCUS:POST_COMMIT:END";
const LOCUSD_BIN_ENV: &str = "LOCUSD_BIN";
const MAX_FILES_IN_MEMORY: usize = 200;

/// Manage Git hook based memory ingestion.
#[derive(Parser, Debug)]
pub struct HookCmd {
    #[command(subcommand)]
    action: HookAction,
}

#[derive(Subcommand, Debug)]
enum HookAction {
    /// Install post-commit hook integration
    Install {
        /// Project path (default: current directory)
        #[arg(long, value_name = "DIR")]
        path: Option<PathBuf>,
    },
    /// Uninstall post-commit hook integration
    Uninstall {
        /// Project path (default: current directory)
        #[arg(long, value_name = "DIR")]
        path: Option<PathBuf>,
    },
    /// Internal: ingest commit metadata into Locus (called by Git hook)
    #[command(name = "run-post-commit", hide = true)]
    RunPostCommit {
        /// Project path (default: current directory)
        #[arg(long, value_name = "DIR")]
        path: Option<PathBuf>,
        /// Explicit commit SHA (default: HEAD)
        #[arg(long)]
        commit: Option<String>,
    },
    /// Inject a context brief for a host lifecycle hook event (U-015)
    Context {
        /// Host adapter to use (default: claude-code)
        #[arg(long, default_value = "claude-code")]
        host: String,
        /// Override the namespace (default: derived from the hook payload `cwd`)
        #[arg(long)]
        namespace: Option<String>,
        /// Override the query; bypasses the adapter-derived query
        #[arg(long)]
        query: Option<String>,
        /// Default-query strategy when there is no query: summary | none
        #[arg(long, default_value = "summary")]
        strategy: String,
        /// Token budget for the brief (default: 200)
        #[arg(long, default_value = "200")]
        token_budget: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

impl HookCmd {
    pub fn run(self) -> Result<()> {
        match self.action {
            HookAction::Install { path } => install(path),
            HookAction::Uninstall { path } => uninstall(path),
            HookAction::RunPostCommit { path, commit } => run_post_commit(path, commit),
            HookAction::Context {
                host,
                namespace,
                query,
                strategy,
                token_budget,
                json,
            } => run_context(host, namespace, query, strategy, token_budget, json),
        }
    }
}

fn install(path: Option<PathBuf>) -> Result<()> {
    let repo = resolve_repo(path)?;
    let hook_path = repo.hooks_dir.join("post-commit");

    let block = hook_block(&repo.root)?;

    if hook_path.is_file() {
        let existing = fs::read_to_string(&hook_path)
            .with_context(|| format!("failed reading {}", hook_path.display()))?;
        if existing.contains(START_MARKER) {
            println!("locus hook already installed at {}", hook_path.display());
            return Ok(());
        }
        let mut merged = existing;
        if !merged.ends_with('\n') {
            merged.push('\n');
        }
        merged.push('\n');
        merged.push_str(&block);
        if !merged.ends_with('\n') {
            merged.push('\n');
        }
        write_hook_file(&hook_path, &merged)?;
    } else {
        let body = format!("#!/usr/bin/env sh\n\n{}\n", block);
        write_hook_file(&hook_path, &body)?;
    }

    println!("Installed post-commit hook: {}", hook_path.display());
    Ok(())
}

fn uninstall(path: Option<PathBuf>) -> Result<()> {
    let repo = resolve_repo(path)?;
    let hook_path = repo.hooks_dir.join("post-commit");

    if !hook_path.exists() {
        println!("No post-commit hook found at {}", hook_path.display());
        return Ok(());
    }

    let existing = fs::read_to_string(&hook_path)
        .with_context(|| format!("failed reading {}", hook_path.display()))?;
    if !existing.contains(START_MARKER) {
        println!("No Locus hook block found at {}", hook_path.display());
        return Ok(());
    }

    let cleaned = remove_locus_block(&existing);
    if cleaned.trim().is_empty() {
        fs::remove_file(&hook_path)
            .with_context(|| format!("failed removing {}", hook_path.display()))?;
        println!("Removed post-commit hook {}", hook_path.display());
    } else {
        write_hook_file(&hook_path, &cleaned)?;
        println!("Uninstalled Locus block from {}", hook_path.display());
    }

    Ok(())
}

fn run_post_commit(path: Option<PathBuf>, commit: Option<String>) -> Result<()> {
    let repo = resolve_repo(path)?;

    let commit_sha = match commit {
        Some(c) if !c.trim().is_empty() => c,
        _ => git_output(&repo.root, &["rev-parse", "--verify", "HEAD"])?,
    };

    let message = git_output(&repo.root, &["log", "-1", "--pretty=%B", &commit_sha])?;
    let files_raw = git_output(
        &repo.root,
        &["show", "--name-only", "--pretty=format:", &commit_sha],
    )?;

    let mut files: Vec<String> = files_raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    if files.len() > MAX_FILES_IN_MEMORY {
        files.truncate(MAX_FILES_IN_MEMORY);
    }

    let subject = message
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .unwrap_or("commit")
        .to_string();

    let short = short_sha(&commit_sha);
    let title = format!("Commit {short}: {subject}");
    let namespace = format!("project:{}", sanitize_name(&repo.project_name));

    let mut content = String::new();
    content.push_str("Commit metadata captured from Git post-commit hook.\n\n");
    content.push_str(&format!("Commit: {commit_sha}\n\n"));
    content.push_str("Message:\n");
    content.push_str(message.trim());
    content.push_str("\n\nChanged files:\n");
    if files.is_empty() {
        content.push_str("- (none)\n");
    } else {
        for path in &files {
            content.push_str("- ");
            content.push_str(path);
            content.push('\n');
        }
    }

    // Send commit metadata through the daemon IPC path.
    let paths = Paths::resolve()?;
    let client = DaemonClient::new(paths.endpoint().clone());
    let daemon_bin = locate_daemon_binary()?;
    client
        .connect_or_spawn(&daemon_bin, paths.data_dir())
        .map_err(|e| anyhow!(e.to_string()))?;

    let payload = RememberRequest {
        namespace: Some(namespace),
        memory_type: "code".to_string(),
        title,
        content,
        entities: vec!["git".to_string(), "commit".to_string(), short.to_string()],
        importance: 60,
        source: Some("git:post-commit".to_string()),
        allow_secret: false,
    };

    let req = Request::new(
        format!("hook-{}", short),
        command::REMEMBER,
        serde_json::to_value(payload)?,
    );

    let resp = client.request(&req)?;
    if !resp.ok {
        let msg = resp
            .error
            .map(|e| e.message)
            .unwrap_or_else(|| "remember request failed".to_string());
        bail!(msg);
    }

    let payload = resp
        .payload
        .ok_or_else(|| anyhow!("remember response missing payload"))?;
    let _decoded: RememberResponse = serde_json::from_value(payload)?;

    Ok(())
}

/// Run pre-reasoning context injection for a host lifecycle hook event.
///
/// Reads the host's hook payload from stdin, translates it through the chosen
/// adapter, and prints a compressed context brief. This is a fast, read-only
/// path. On any failure it degrades gracefully: the error goes to stderr and
/// `NO_RELEVANT_MEMORY` is printed so the host event is never blocked.
fn run_context(
    host: String,
    namespace_override: Option<String>,
    query_override: Option<String>,
    strategy: String,
    token_budget: usize,
    json: bool,
) -> Result<()> {
    let trigger = match build_trigger(
        host,
        namespace_override,
        query_override,
        strategy,
        token_budget,
    ) {
        Ok(trigger) => trigger,
        Err(err) => {
            eprintln!("locus hook context: {err}");
            print_brief(NO_RELEVANT_MEMORY, json);
            return Ok(());
        }
    };

    let store = match Paths::resolve().and_then(|paths| Store::open_at(paths.db_file())) {
        Ok(store) => store,
        Err(err) => {
            eprintln!("locus hook context: {err}");
            print_brief(NO_RELEVANT_MEMORY, json);
            return Ok(());
        }
    };

    match inject_context(&store, &trigger) {
        Ok(brief) => print_brief(&brief, json),
        Err(err) => {
            eprintln!("locus hook context: {err}");
            print_brief(NO_RELEVANT_MEMORY, json);
        }
    }

    Ok(())
}

fn build_trigger(
    host: String,
    namespace_override: Option<String>,
    query_override: Option<String>,
    strategy: String,
    token_budget: usize,
) -> Result<InjectTrigger> {
    let namespace = namespace_override.filter(|ns| !ns.trim().is_empty());

    if let Some(query) = query_override {
        let query = query.trim().to_string();
        if query.is_empty() {
            bail!("--query must not be empty");
        }
        return Ok(InjectTrigger {
            namespace,
            query: Some(query),
            strategy: DefaultQueryStrategy::Summary,
            token_budget,
        });
    }

    let mut payload = String::new();
    io::stdin()
        .read_to_string(&mut payload)
        .context("failed to read hook payload from stdin")?;

    let adapter = adapter_for(&host).map_err(anyhow::Error::from)?;
    let mut trigger = adapter
        .translate(payload.trim())
        .map_err(anyhow::Error::from)?;
    trigger.namespace = namespace.or(trigger.namespace);
    trigger.strategy = DefaultQueryStrategy::parse(&strategy)?;
    trigger.token_budget = token_budget;
    Ok(trigger)
}

fn print_brief(brief: &str, json: bool) {
    if json {
        println!(
            "{{\"status\":\"ok\",\"brief\":{}}}",
            serde_json::to_string(brief).expect("serializing brief")
        );
    } else {
        println!("{brief}");
    }
}

#[derive(Debug)]
struct RepoPaths {
    root: PathBuf,
    hooks_dir: PathBuf,
    project_name: String,
}

fn resolve_repo(path: Option<PathBuf>) -> Result<RepoPaths> {
    let base = path
        .unwrap_or_else(|| std::env::current_dir().expect("current dir"))
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("."));

    let root = git_output_path(&base, &["rev-parse", "--show-toplevel"])?;
    let hooks = git_output_path(&base, &["rev-parse", "--git-path", "hooks"])?;
    let hooks_dir = if hooks.is_absolute() {
        hooks
    } else {
        root.join(hooks)
    };
    let project_name = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_string();

    Ok(RepoPaths {
        root,
        hooks_dir,
        project_name,
    })
}

fn git_output_path(root: &Path, args: &[&str]) -> Result<PathBuf> {
    let text = git_output(root, args)?;
    Ok(PathBuf::from(text))
}

fn git_output(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        bail!("git {} failed: {}", args.join(" "), err);
    }

    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn hook_block(project_root: &Path) -> Result<String> {
    let exe = std::env::current_exe().context("failed to resolve current locus binary")?;
    let exe_quoted = sh_quote(exe.to_string_lossy().as_ref());
    let root_quoted = sh_quote(project_root.to_string_lossy().as_ref());

    Ok(format!(
        "{START_MARKER}\nif [ -x {exe_quoted} ]; then\n  ({exe_quoted} hook run-post-commit --path {root_quoted} >/dev/null 2>&1 &) || true\nelif command -v locus >/dev/null 2>&1; then\n  (locus hook run-post-commit --path {root_quoted} >/dev/null 2>&1 &) || true\nfi\n{END_MARKER}"
    ))
}

fn remove_locus_block(content: &str) -> String {
    let mut out = String::new();
    let mut skip = false;

    for line in content.lines() {
        if line.trim() == START_MARKER {
            skip = true;
            continue;
        }
        if line.trim() == END_MARKER {
            skip = false;
            continue;
        }
        if !skip {
            out.push_str(line);
            out.push('\n');
        }
    }

    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }

    out
}

fn write_hook_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content).with_context(|| format!("failed writing {}", path.display()))?;
    set_hook_executable(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_hook_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_hook_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn short_sha(sha: &str) -> &str {
    let trimmed = sha.trim();
    let len = trimmed.len().min(12);
    &trimmed[..len]
}

fn sanitize_name(raw: &str) -> String {
    let s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();

    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "project".to_string()
    } else {
        s
    }
}

fn sh_quote(input: &str) -> String {
    let escaped = input.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn locate_daemon_binary() -> Result<PathBuf> {
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
